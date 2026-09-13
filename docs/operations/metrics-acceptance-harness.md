# Metrics acceptance harness

`tests/metrics_acceptance_harness.py` is a loopback-only smoke harness for the
remaining Issue #307/#217 acceptance slice. It renders metrics through the
production `aether-runtime` billing recorders, serves that payload from an
isolated `/metrics` target, checks Prometheus exposition-family metadata, and
posts `firing` followed by `resolved` payloads to a local Alertmanager fixture.

Run from the repository root:

```bash
python3 tests/metrics_acceptance_harness.py
python3 tests/metrics_acceptance_harness.py --token metrics-fixture-token
# Gateway ingress route; the caller must hold admin:monitoring:read.
python3 tests/metrics_acceptance_harness.py --endpoint http://127.0.0.1:9090/_gateway/metrics --token "$METRICS_TOKEN"
```

With `--endpoint`, the harness performs a real HTTP scrape and requires
`text/plain` Prometheus content. The deployed gateway endpoint is
`/_gateway/metrics` and is protected by the `admin:monitoring:read` permission;
the local fixture itself intentionally serves `/metrics` and accepts the
fixture bearer token. The endpoint is expected to be loopback or a deliberately
isolated test deployment. No token is logged. The local mode proves the
authentication negative/positive boundary and the local Alertmanager
delivery/recovery sequence.

This is intentionally not deployment evidence: it does not run Prometheus,
exercise a production ingress, or prove delivery through an external
Alertmanager. The billing fixture injects failures through real producer
recorders, but it does not cover every production failure branch (usage,
video-task, enrichment, retry and DLQ paths must still be fault-injected in a
staging deployment before closing #307/#217). Preserve the command output and
deployment scrape/webhook logs as separate acceptance artifacts.
