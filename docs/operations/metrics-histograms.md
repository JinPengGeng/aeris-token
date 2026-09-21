# Histogram latency metrics

Delivered under #492 per `docs/adr/metrics-histogram-evaluation.md`: the
self-built Prometheus renderer (`crates/aether-runtime/base/src/metrics.rs`)
now supports a first-class `histogram` type. Each histogram is exported in the
standard text format as cumulative `<name>_bucket{le="..."}` lines plus
`<name>_sum` and `<name>_count`. Bucket counts are stored sparsely (one
exclusive bucket per observation, atomically) and rendered cumulatively, so
the hot path costs one atomic increment.

## Bucket layout

All histograms share the sparse log-bucket layout from the ADR, expressed in seconds
(promtool lint requires base units):

`0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, +Inf`

Ten finite buckets cover the 5ms–5s gateway/upstream latency domain; `+Inf` is
the implicit observation total and always equals `_count`. Memory per
histogram is 10 × 8 bytes of counters plus one atomic sum — the ADR's
"< 100 new series" budget is met: 5 histograms × ~12 series each.

## Metrics

Scrape path `/_gateway/metrics` (authenticated, `admin:monitoring:read`).
Names below are unprefixed suffixes; a deployment may prepend its metrics
namespace (e.g. `aether_gateway_`).

| Metric | Labels (fixed) | Meaning | Recorded at |
| --- | --- | --- | --- |
| `request_duration_seconds` | none | Terminal gateway request end-to-end latency distribution. | `request_metrics::RequestMetrics::record` (terminal boundary, #482 RED module) |
| `upstream_first_token_seconds` | none | Request start → first upstream stream payload byte (TTFT proxy). | `execution_runtime/stream/execution.rs` `observe_upstream_chunk`, first captured stream event |
| `ws_connect_duration_seconds` | `outcome=success\|failure` | DNS → TCP → TLS → HTTP 101 Upgrade for an upstream Responses WebSocket. | `handlers/proxy/websocket/responses/upstream.rs` bind path |
| `billing_settlement_duration_seconds` | `component=usage`, `operation=settlement` | Terminal usage settlement duration (entry to completion, all outcomes). | shared usage-settlement wrapper `settle_usage_with_reconciled_cost` |
| `request_candidate_queue_dwell_seconds` | none | Normal-lane enqueue → flush dwell of request candidate persistence records. | `request_candidate_queue.rs` `collect_ready_normal_batch` |

Label discipline is unchanged: only the fixed values above are emitted.
Request IDs, users, models, raw URLs and error text are never labels.

## Query examples

p95 request latency over 5 minutes:

```promql
histogram_quantile(
  0.95,
  sum by (le) (rate(request_duration_seconds_bucket[5m]))
)
```

Share of upstream TTFT slower than 2.5s:

```promql
sum(rate(upstream_first_token_seconds_bucket{le="+Inf"}[5m]))
  - sum(rate(upstream_first_token_seconds_bucket{le="2.5"}[5m]))
```

Failed WS connects per second (connect-path failures, not turn failures):

```promql
sum(rate(ws_connect_duration_seconds_count{outcome="failure"}[5m]))
```

Settlement path regression (average settlement duration; seconds keep the promtool unit lint clean):

```promql
sum(rate(billing_settlement_duration_seconds_sum[5m]))
  / sum(rate(billing_settlement_duration_seconds_count[5m]))
```

Queue backpressure signal (slow flush: dwell above 500ms per second):

```promql
sum(rate(request_candidate_queue_dwell_seconds_bucket{le="+Inf"}[5m]))
  - sum(rate(request_candidate_queue_dwell_seconds_bucket{le="0.5"}[5m]))
```

Histograms are process-local and reset on restart; use `rate()`/`increase()`
and tolerate resets, like the existing counters.
