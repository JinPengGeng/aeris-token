# Gateway billing and failure metrics contract

This contract is the runtime-owned slice for Issue #307 (217-A/B). The gateway
exports the samples from `/_gateway/metrics`. Scraping requires an authenticated
operational identity with the `admin:monitoring:read` permission. A deployment
may prefix names with its configured metrics namespace; the unprefixed names
below are the stable suffixes used by alert rules.

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

The request RED contract was delivered in #306. `request_total` has fixed label
keys `route_class`, `status_class`, `provider`, and `outcome`;
`request_errors_total` omits `outcome` because it counts only the `error`
terminal outcome. `provider` is a normalized provider **type**, never an
arbitrary configured provider ID. Unrecognized types become `unknown`.
These are terminal gateway outcomes, including gateway failures; the 5xx rule
does not by itself establish that a provider attempt failed.

The shared Prometheus text renderer declares HELP/TYPE once per metric family
and retains every labeled sample. The real billing exporter output is checked
with Prometheus `promtool`; the alert fixture tests pending, firing, counter
reset and recovery behavior. See the runbook for commands and evidence limits.
