# Gateway billing and failure metrics contract

This contract is the runtime-owned slice for Issue #307 (217-A/B). The gateway
exports the samples from the existing `/metrics` endpoint. A deployment may
prefix names with its configured metrics namespace; the unprefixed names below
are the stable suffixes used by alert rules.

| Metric | Type | Fixed labels | Meaning and owner |
| --- | --- | --- | --- |
| `billing_enrichment_failures_total` | counter | `component=usage`, `operation=enrichment` | A terminal usage event could not be enriched before persistence. `aether-usage-runtime` owns the event points. |
| `billing_settlement_failures_total` | counter | `component=usage`, `operation=settlement` | A terminal usage settlement failed after the usage record was written. `aether-usage-runtime` owns the event points. |
| `billing_video_task_settlement_failures_total` | counter | `component=video_task`, `operation=settlement` | A video task finalizer could not settle its terminal usage event. Gateway video-task finalizer owns the event point. |
| `billing_fail_open_total` | counter | `component=gateway`, `operation=daily_quota` or `operation=rpm` | A billing or abuse guard allowed a request after its runtime dependency failed. Gateway frontdoor owns the event points. |

Only the listed fixed values are emitted. Request IDs, users, models, raw URLs,
error text and credentials are never labels. Counters are process-local and
monotonic; a restart resets them, so alerting should use `rate()` or
`increase()` and tolerate a reset. A duplicate event is counted only when the
corresponding failure branch is entered; retries that do not enter a failure
branch do not increment a counter.

The request RED contract and provider status dimensions are intentionally owned
by Issue #306. The alert examples reference that future stable contract rather
than introducing a second provider metric family here.
