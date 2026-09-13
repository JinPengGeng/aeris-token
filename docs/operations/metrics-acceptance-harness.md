# Metrics acceptance harness

`tests/metrics_acceptance_harness.py` is a deterministic smoke harness for the
remaining Issue #307/#217 acceptance slice. Its default mode is loopback-only;
explicit endpoint flags can target an isolated or staging deployment. It renders metrics through the
production `aether-runtime` billing recorders, serves that payload from an
isolated `/metrics` target, checks Prometheus exposition-family metadata, and
posts `firing` followed by `resolved` payloads to a local Alertmanager fixture.

Run from the repository root:

```bash
python3 tests/metrics_acceptance_harness.py
python3 tests/metrics_acceptance_harness.py --token metrics-fixture-token
python3 tests/metrics_acceptance_harness.py --endpoint http://127.0.0.1:9090/_gateway/metrics --token "$METRICS_TOKEN"
python3 tests/metrics_acceptance_harness.py \
  --endpoint http://127.0.0.1:9090/_gateway/metrics --token "$METRICS_TOKEN" \
  --alertmanager-endpoint http://127.0.0.1:9093/api/v1/alerts \
  --alertmanager-token "$ALERTMANAGER_TOKEN"
```

With `--endpoint`, the harness performs a real HTTP scrape and requires
`text/plain` Prometheus content. The endpoint is expected to be loopback or a
deliberately isolated test deployment. No token is logged. The local mode
proves the authentication negative/positive boundary and local Alertmanager
delivery/recovery; endpoint mode performs the same checks against the supplied
HTTP targets.

This is intentionally not production deployment evidence: it does not run
Prometheus or exercise a production ingress. The billing fixture injects failures through real producer
recorders, but it does not cover every production failure branch (usage,
video-task, enrichment, retry and DLQ paths must still be fault-injected in a
staging deployment before closing #307/#217). Preserve the command output and
deployment scrape/webhook logs as separate acceptance artifacts.

When `--alertmanager-endpoint` is supplied, the harness POSTs deterministic
`firing` and `resolved` payloads and requires HTTP 2xx for both requests. The
endpoint must be an isolated or staging Alertmanager API; bearer credentials
are never printed. HTTP success proves API acceptance only, not notification
delivery to a receiver. Capture Alertmanager notification logs (or query its
API) and Prometheus target/alert status as independent evidence. The harness
deliberately does not contact a production endpoint.
