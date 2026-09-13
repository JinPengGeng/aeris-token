#!/usr/bin/env python3
"""Run real Prometheus -> Alertmanager -> webhook delivery on owned loopback ports."""

from __future__ import annotations

import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode
from urllib.request import ProxyHandler, Request, build_opener


TOKEN = "metrics-fixture-token"
ALERT = "AetherBillingGuardFailOpen"
METRIC = "aether_gateway_billing_fail_open_total"
OPERATIONS = {"daily_quota", "rpm"}
ROOT = Path(__file__).resolve().parent.parent
LOCAL_HTTP = build_opener(ProxyHandler({}))


class Fixture:
    def __init__(self, payload: bytes):
        self.payload = payload
        self.deliveries: list[dict] = []
        self.lock = threading.Lock()

    def handler(self):
        fixture = self

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):  # noqa: N802
                if self.path != "/metrics":
                    self.send_error(404)
                    return
                if self.headers.get("Authorization") != f"Bearer {TOKEN}":
                    self.send_error(401)
                    return
                with fixture.lock:
                    payload = fixture.payload
                self.send_response(200)
                self.send_header("Content-Type", "text/plain; version=0.0.4")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

            def do_POST(self):  # noqa: N802
                if self.path != "/alerts":
                    self.send_error(404)
                    return
                try:
                    size = int(self.headers.get("Content-Length", "0"))
                    if not 0 < size <= 65536:
                        raise ValueError("invalid webhook size")
                    payload = json.loads(self.rfile.read(size))
                    if payload.get("version") != "4" or not isinstance(payload.get("alerts"), list):
                        raise ValueError("expected Alertmanager webhook v4")
                except (ValueError, AttributeError):
                    self.send_error(400)
                    return
                with fixture.lock:
                    fixture.deliveries.append(payload)
                self.send_response(200)
                self.end_headers()

            def log_message(self, *_args):
                pass

        return Handler

    def observed(self, status: str) -> bool:
        with self.lock:
            operations = {
                alert["labels"].get("operation")
                for delivery in self.deliveries
                for alert in delivery["alerts"]
                if alert.get("status") == status
                and alert.get("labels", {}).get("alertname") == ALERT
                and alert["labels"].get("component") == "gateway"
            }
        return OPERATIONS <= operations


class LoopbackServer(ThreadingHTTPServer):
    # Avoid reverse DNS on systems where localhost DNS is intercepted.
    def server_bind(self):
        self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.socket.bind(self.server_address)
        self.server_address = self.socket.getsockname()
        self.server_name = "localhost"
        self.server_port = self.server_address[1]


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def get(url: str, token: str | None = None) -> bytes:
    headers = {"Authorization": f"Bearer {token}"} if token else {}
    with LOCAL_HTTP.open(Request(url, headers=headers), timeout=3) as response:
        return response.read()


def query(base: str, expression: str) -> list:
    result = json.loads(get(f"{base}/api/v1/query?{urlencode({'query': expression})}"))
    if result["status"] != "success":
        raise AssertionError(f"Prometheus query failed: {expression}")
    return result["data"]["result"]


def sample_is(base: str, expression: str, value: float) -> bool:
    results = query(base, expression)
    return bool(results) and all(float(item["value"][1]) == value for item in results)


def wait_for(label, predicate, processes, timeout=90):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        for name, process in processes:
            if process.poll() is not None:
                raise AssertionError(f"{name} exited with {process.returncode}; inspect evidence logs")
        try:
            if predicate():
                return
        except (URLError, TimeoutError, ConnectionError):
            pass
        time.sleep(0.2)
    raise AssertionError(f"timed out waiting for {label}")


def stop_process(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


def start(stack, command, log_path):
    log = stack.enter_context(log_path.open("wb"))
    process = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT)
    stack.callback(stop_process, process)
    return process


def write_json(path: Path, content):
    path.write_text(json.dumps(content, indent=2) + "\n")


def fixture_metrics(healthy: bool) -> bytes:
    command = ["cargo", "run", "--locked", "--quiet", "-p", "aether-runtime",
               "--example", "prometheus_billing_fixture"]
    if healthy:
        command += ["--", "--healthy"]
    return subprocess.check_output(command, cwd=ROOT, timeout=300)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prometheus-bin", default="prometheus")
    parser.add_argument("--promtool-bin", default="promtool")
    parser.add_argument("--alertmanager-bin", default="alertmanager")
    parser.add_argument("--evidence-dir", type=Path, help="new directory for retained logs and results")
    args = parser.parse_args()
    for binary in (args.prometheus_bin, args.promtool_bin, args.alertmanager_bin):
        if shutil.which(binary) is None:
            parser.error(f"required executable not found: {binary}")
    evidence = args.evidence_dir
    if evidence:
        evidence.mkdir(parents=True, exist_ok=False)
    else:
        evidence = Path(tempfile.mkdtemp(prefix="aeris-metrics-evidence-"))
    print(f"Evidence: {evidence}", flush=True)

    healthy, failed = fixture_metrics(True), fixture_metrics(False)
    for name, payload in (("healthy", healthy), ("failed", failed)):
        (evidence / f"{name}.prom").write_bytes(payload)
        result = subprocess.run([args.promtool_bin, "check", "metrics"], input=payload,
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=15)
        (evidence / f"{name}-parse.log").write_bytes(result.stdout)
        result.check_returncode()
    for name, binary in (("prometheus", args.prometheus_bin), ("alertmanager", args.alertmanager_bin)):
        (evidence / f"{name}-version.txt").write_bytes(
            subprocess.check_output([binary, "--version"], stderr=subprocess.STDOUT, timeout=10))

    fixture = Fixture(healthy)
    try:
        with ExitStack() as stack:
            state_dir = Path(stack.enter_context(tempfile.TemporaryDirectory(prefix="aeris-metrics-state-")))
            server = LoopbackServer(("127.0.0.1", 0), fixture.handler())
            stack.callback(server.server_close)
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            stack.callback(thread.join, 5)
            stack.callback(server.shutdown)
            fixture_host = f"127.0.0.1:{server.server_port}"
            for token in (None, "incorrect-fixture-token"):
                try:
                    get(f"http://{fixture_host}/metrics", token)
                except HTTPError as error:
                    assert error.code == 401
                else:
                    raise AssertionError("fixture accepted missing or wrong bearer credentials")
            assert get(f"http://{fixture_host}/metrics", TOKEN) == healthy

            am_port, prom_port = free_port(), free_port()
            while prom_port == am_port:
                prom_port = free_port()
            am_url, prom_url = f"http://127.0.0.1:{am_port}", f"http://127.0.0.1:{prom_port}"
            # JSON is a YAML subset accepted by both binaries. No production config is edited.
            am_config = evidence / "alertmanager.json"
            write_json(am_config, {
                "route": {"receiver": "fixture", "group_by": ["alertname", "component", "operation"],
                          "group_wait": "0s", "group_interval": "1s", "repeat_interval": "1h"},
                "receivers": [{"name": "fixture", "webhook_configs": [
                    {"url": f"http://{fixture_host}/alerts", "send_resolved": True}]}],
            })
            rules = evidence / "live-rules.json"
            write_json(rules, {"groups": [{"name": "aether-transport-drill", "rules": [{
                "alert": ALERT,
                "expr": f"sum by (component, operation) (increase({METRIC}[10s])) > 0",
                "for": "2s", "labels": {"severity": "critical"},
            }]}]})
            prom_config = evidence / "prometheus.json"
            write_json(prom_config, {
                "global": {"scrape_interval": "1s", "evaluation_interval": "1s", "scrape_timeout": "500ms"},
                "rule_files": [str(rules.resolve())],
                "scrape_configs": [
                    {"job_name": "aether", "authorization": {"type": "Bearer", "credentials": TOKEN},
                     "static_configs": [{"targets": [fixture_host]}]},
                    {"job_name": "missing_token", "static_configs": [{"targets": [fixture_host]}]},
                ],
                "alerting": {"alertmanagers": [{"api_version": "v2", "static_configs": [
                    {"targets": [f"127.0.0.1:{am_port}"]}]}]},
            })
            am = start(stack, [args.alertmanager_bin, f"--config.file={am_config.resolve()}",
                              f"--web.listen-address=127.0.0.1:{am_port}", "--cluster.listen-address=",
                              f"--storage.path={state_dir / 'alertmanager'}"], evidence / "alertmanager.log")
            processes = [("Alertmanager", am)]
            wait_for("Alertmanager readiness", lambda: get(f"{am_url}/-/ready"), processes)
            prom = start(stack, [args.prometheus_bin, f"--config.file={prom_config.resolve()}",
                                f"--web.listen-address=127.0.0.1:{prom_port}",
                                f"--storage.tsdb.path={state_dir / 'prometheus'}"],
                         evidence / "prometheus.log")
            processes.append(("Prometheus", prom))
            wait_for("Prometheus readiness", lambda: get(f"{prom_url}/-/ready"), processes)
            wait_for("authenticated scrape", lambda: sample_is(prom_url, 'up{job="aether"}', 1), processes)
            wait_for("rejected unauthenticated scrape",
                     lambda: sample_is(prom_url, 'up{job="missing_token"}', 0), processes)
            wait_for("two baseline samples", lambda: bool(query(
                prom_url, f'min(count_over_time({METRIC}{{job="aether"}}[10s])) >= 2')), processes)
            assert sample_is(prom_url, f'{METRIC}{{job="aether"}}', 0)
            with fixture.lock:
                assert not fixture.deliveries, "healthy baseline unexpectedly notified"
                fixture.payload = failed
            wait_for("producer failure scrape", lambda: sample_is(
                prom_url, f'{METRIC}{{job="aether"}}', 1), processes)
            wait_for("real Alertmanager firing for daily_quota and rpm",
                     lambda: fixture.observed("firing"), processes)
            # Hold counters at one. The increase ages out without resetting the counter.
            wait_for("real Alertmanager resolution for daily_quota and rpm",
                     lambda: fixture.observed("resolved"), processes)
            with fixture.lock:
                for operation in OPERATIONS:
                    states = [alert["status"] for delivery in fixture.deliveries
                              for alert in delivery["alerts"]
                              if alert.get("labels", {}).get("alertname") == ALERT
                              and alert["labels"].get("operation") == operation]
                    assert states[0] == "firing" and states[-1] == "resolved", states
            assert not query(prom_url, f'ALERTS{{alertname="{ALERT}",alertstate="firing"}}')
            (evidence / "targets.json").write_bytes(get(f"{prom_url}/api/v1/targets"))
            write_json(evidence / "result.json", {
                "status": "passed", "operations": sorted(OPERATIONS),
                "delivery": "Prometheus -> Alertmanager v2 API -> webhook v4",
                "test_timing": {"increase_window": "10s", "for": "2s"},
                "production_rule_timing": "validated separately by promtool test rules",
            })
    finally:
        with fixture.lock:
            write_json(evidence / "webhooks.json", fixture.deliveries)
    print("PASS: real scrape, rule evaluation, Alertmanager firing and resolved webhooks")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (AssertionError, OSError, subprocess.SubprocessError) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)
