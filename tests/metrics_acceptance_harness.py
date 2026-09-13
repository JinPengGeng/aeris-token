#!/usr/bin/env python3
"""Isolated metrics scrape and Alertmanager delivery acceptance harness.

The harness deliberately uses loopback-only HTTP fixtures. It never contacts a
production endpoint or an external Alertmanager.
"""

from __future__ import annotations

import argparse
import json
import re
import socket
import subprocess
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.error import HTTPError
from urllib.request import Request, urlopen


TOKEN = "metrics-fixture-token"


class LoopbackHTTPServer(ThreadingHTTPServer):
    """Avoid reverse-DNS lookup when binding deterministic local fixtures."""

    def server_bind(self):
        self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.socket.bind(self.server_address)
        self.server_address = self.socket.getsockname()
        self.server_name = "localhost"
        self.server_port = self.server_address[1]


class MetricsHandler(BaseHTTPRequestHandler):
    payload = b""
    token_required = False

    def do_GET(self):  # noqa: N802
        if self.path != "/metrics":
            self.send_error(404)
            return
        if self.token_required and self.headers.get("Authorization") != f"Bearer {TOKEN}":
            self.send_error(401)
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/plain; version=0.0.4")
        self.send_header("Content-Length", str(len(self.payload)))
        self.end_headers()
        self.wfile.write(self.payload)

    def log_message(self, *_args):
        pass


class AlertmanagerHandler(BaseHTTPRequestHandler):
    deliveries = []

    def do_POST(self):  # noqa: N802
        length = int(self.headers.get("Content-Length", "0"))
        try:
            self.deliveries.append(json.loads(self.rfile.read(length)))
        except (json.JSONDecodeError, ValueError):
            self.send_error(400)
            return
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"ok")

    def log_message(self, *_args):
        pass


def scrape(url: str, token: str | None) -> str:
    headers = {"Authorization": f"Bearer {token}"} if token else {}
    with urlopen(Request(url, headers=headers), timeout=5) as response:
        content_type = response.headers.get("Content-Type", "")
        if "text/plain" not in content_type:
            raise AssertionError(f"unexpected metrics content type: {content_type!r}")
        if response.status != 200:
            raise AssertionError(f"metrics scrape returned HTTP {response.status}")
        return response.read().decode("utf-8")


def post_alert(url: str, state: str, token: str | None = None) -> None:
    payload = [{"status": state, "labels": {"alertname": "AetherBillingGuardFailOpen", "severity": "critical"}}]
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    request = Request(url, data=json.dumps(payload).encode(), headers=headers, method="POST")
    with urlopen(request, timeout=5) as response:
        if not 200 <= response.status < 300:
            raise AssertionError(f"Alertmanager endpoint returned HTTP {response.status}")


def fixture_metrics() -> bytes:
    command = ["cargo", "run", "--locked", "--quiet", "-p", "aether-runtime", "--example", "prometheus_billing_fixture"]
    return subprocess.check_output(command, text=True).encode()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", help="existing /metrics URL; defaults to local exporter fixture")
    parser.add_argument("--token", help="Bearer token for an authenticated endpoint")
    parser.add_argument(
        "--alertmanager-endpoint",
        help="Alertmanager /api/v1/alerts URL; defaults to a loopback fixture",
    )
    parser.add_argument("--alertmanager-token", help="Bearer token for --alertmanager-endpoint")
    parser.add_argument("--metrics-file", help="use a previously captured exposition instead of cargo")
    args = parser.parse_args()

    # An external scrape is the source of truth; avoid building the optional
    # local Rust fixture in that mode so the harness can run independently.
    payload = open(args.metrics_file, "rb").read() if args.metrics_file else (None if args.endpoint else fixture_metrics())
    scraped_payload = payload or b""
    required = ["aether_gateway_billing_enrichment_failures_total", "aether_gateway_billing_fail_open_total"]
    if payload is not None:
        for name in required:
            if name.encode() not in payload:
                raise AssertionError(f"producer failure fixture did not emit {name}")
        text = payload.decode()
        for family in required:
            if len(re.findall(rf"^# HELP {re.escape(family)} ", text, re.MULTILINE)) != 1:
                raise AssertionError(f"metric family {family} has duplicate or missing HELP")
            if len(re.findall(rf"^# TYPE {re.escape(family)} ", text, re.MULTILINE)) != 1:
                raise AssertionError(f"metric family {family} has duplicate or missing TYPE")

    metrics_server = None
    if args.endpoint:
        endpoint = args.endpoint
        try:
            scraped_payload = scrape(endpoint, args.token).encode()
        except HTTPError as error:
            if args.token:
                raise AssertionError(f"authenticated scrape failed: HTTP {error.code}") from error
            raise
    else:
        MetricsHandler.payload = payload or b""
        MetricsHandler.token_required = bool(args.token)
        metrics_server = LoopbackHTTPServer(("127.0.0.1", 0), MetricsHandler)
        threading.Thread(target=metrics_server.serve_forever, daemon=True).start()
        endpoint = f"http://127.0.0.1:{metrics_server.server_port}/metrics"
        if args.token:
            try:
                scrape(endpoint, None)
            except HTTPError as error:
                if error.code != 401:
                    raise AssertionError(f"expected unauthenticated scrape to fail with 401, got {error.code}") from error
            else:
                raise AssertionError("unauthenticated scrape unexpectedly succeeded")
        scraped_payload = scrape(endpoint, args.token).encode()

    if args.endpoint:
        for name in required:
            if name.encode() not in scraped_payload:
                raise AssertionError(f"scraped endpoint did not emit {name}")
        scraped_text = scraped_payload.decode()
        for family in required:
            if len(re.findall(rf"^# HELP {re.escape(family)} ", scraped_text, re.MULTILINE)) != 1:
                raise AssertionError(f"scraped family {family} has duplicate or missing HELP")
            if len(re.findall(rf"^# TYPE {re.escape(family)} ", scraped_text, re.MULTILINE)) != 1:
                raise AssertionError(f"scraped family {family} has duplicate or missing TYPE")

    alert_server = None
    if args.alertmanager_endpoint:
        alert_url = args.alertmanager_endpoint
        post_alert(alert_url, "firing", args.alertmanager_token)
        post_alert(alert_url, "resolved", args.alertmanager_token)
        alert_result = "external Alertmanager HTTP acceptance"
    else:
        AlertmanagerHandler.deliveries = []
        alert_server = LoopbackHTTPServer(("127.0.0.1", 0), AlertmanagerHandler)
        threading.Thread(target=alert_server.serve_forever, daemon=True).start()
        alert_url = f"http://127.0.0.1:{alert_server.server_port}/api/v1/alerts"
        post_alert(alert_url, "firing")
        post_alert(alert_url, "resolved")
        if [item[0]["status"] for item in AlertmanagerHandler.deliveries] != ["firing", "resolved"]:
            raise AssertionError("Alertmanager fixture did not observe firing then resolved delivery")
        alert_result = "Alertmanager fixture firing/recovery"

    if metrics_server:
        metrics_server.shutdown()
    if alert_server:
        alert_server.shutdown()
    print(f"metrics scrape/auth, producer fixture, and {alert_result}: PASS")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (AssertionError, subprocess.CalledProcessError) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)
