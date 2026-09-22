# Prometheus / Grafana minimal monitoring stack

Minimal, repository-owned monitoring assets for the Aether gateway (Ref #217).
Alert naming, severity and annotation style follow the logging ADR and the
alert duty index from #482; alert semantics are pinned by promtool fixture
tests in `aether-alerts.test.yml` (run via the metrics acceptance harness,
see `../metrics-acceptance-harness.md`).

| File | Purpose |
| --- | --- |
| `prometheus.yml` | Sample scrape config: gateway `/_gateway/metrics` (job `aether-gateway`, target `app:8084`). |
| `aether-alerts.yml` | Alert rules (~10) based on low-cardinality counters/gauges, not log levels. |
| `aether-alerts.test.yml` | promtool alert-rule tests pinning every rule above. |
| `grafana-dashboard.json` | Minimal dashboard: billing/settlement counters, request RED (route_class/status_class/provider), usage DLQ, billing fail-open. |

Notes:

- All metric names carry the `aether_gateway_` namespace; see
  `../metrics-contract.md`.
- The self-built renderer exposes only counter/gauge plus sparse log-bucket
  histograms; the dashboard derives mean latency as
  `rate(..._sum[5m]) / rate(..._count[5m])` instead of histogram quantiles.
- These files are samples: the repository does not install Prometheus or
  Grafana for you. Mount `prometheus.yml` into a Prometheus container and
  import `grafana-dashboard.json` with a Prometheus datasource.
