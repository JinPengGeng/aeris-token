# Metrics scrape and notification acceptance

The drill runs real Prometheus and Alertmanager processes on owned loopback
ports. Prometheus scrapes the production Rust metrics renderer, evaluates a
bounded alert rule, sends alerts through Alertmanager's v2 API, and Alertmanager
delivers firing and resolved webhook v4 notifications to an isolated receiver.
The harness never posts synthetic alerts itself.

## Run and evidence

Install the same Prometheus 3.14.0 and Alertmanager 0.34.0 versions pinned by
`.github/workflows/prometheus-ci.yml`, verifying the release archive checksums.
Run from the repository root with Rust 1.95 and Python 3:

```sh
python3 tests/metrics_acceptance_harness.py --evidence-dir /tmp/aether-metrics-evidence
```

The evidence directory must be new. Omit the option to allocate one
automatically. For binaries outside PATH, pass `--prometheus-bin`,
`--promtool-bin` and `--alertmanager-bin` with their absolute paths.
No Docker daemon, production credentials, database or external receiver is
needed. Missing tools or failed delivery fail the drill; no mock fallback is
accepted. Child processes are terminated on success or failure, their temporary
data directories are removed, and evidence is retained.

The artifact contains both exporter samples, parser output, tool versions,
effective configs, process logs, final target health, actual webhook payloads
and `result.json`. CI uploads these alongside the existing metrics/rule evidence.
Configs contain only the fixed local fixture token, never a deployment secret.
The Prometheus workflow is called by Rust CI; its result is an explicit input
to the required `Rust CI / check`, so a missing tool or failed notification
blocks that gate. The standalone manual dispatch remains available.

## What is verified

1. The runtime example renders a zero-counter baseline using `--healthy` and
   the default failure sample through the real billing recorders. Promtool
   parses both expositions.
2. Missing and wrong bearer credentials are rejected by the owned metrics
   fixture. Real Prometheus reports the authenticated target up and an otherwise
   identical target without credentials down.
3. After at least two zero samples, the fixture exposes the failure sample.
   Prometheus observes increments for both `daily_quota` and `rpm`.
4. Alertmanager delivers firing notifications for both operations. Counters
   remain at one while their increases age out; the receiver must then observe
   resolved notifications, in that order. Prometheus must have no firing
   instances of the drill alert at the end.

This transport drill uses the fail-open rule's aggregation with a **10-second
increase window and 2-second for duration** in a temporary config. This keeps
the live test short. The shipped rule's actual 10-minute window, 2-minute for
duration, thresholds and other alert families continue to be checked unchanged
by `promtool check rules` and `promtool test rules` in the preceding CI step.
The accelerated drill does not prove those production timing values.

## Review decision and remaining scope

The earlier smoke harness posted JSON to a Python server named Alertmanager
and checked its own captured requests. That did not exercise Prometheus, the
Alertmanager API or notification delivery. The proposed external `/api/v1/alerts`
extension also used synthetic status payloads incompatible with the current
Alertmanager v2 lifecycle. Replace that approach with the real local chain.

This tests producer recorders and transport, not every Gateway failure caller.
It does not validate Gateway RBAC: the actual Gateway metrics route remains
`/_gateway/metrics` with `admin:monitoring:read`, while the owned fixture uses
`/metrics`. Production usage/video/enrichment/retry/DLQ event-path coverage and
operational deployment acceptance remain separately tracked by #307/#217.
No maintained Prometheus/Alertmanager deployment is added to default Compose.
Rollback of the drill is a code/CI revert; no application schema changes.
