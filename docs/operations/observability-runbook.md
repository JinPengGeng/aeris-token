# Observability and billing failure runbook

The examples below assume the gateway metrics namespace is `aether_gateway`.
Use the namespace configured by the deployment when it differs. These counters
contain no credentials, request IDs or user data.

## First response

1. Confirm the alert is from a live gateway and note its UTC start time.
2. Query the matching counter with `increase(...[10m])`; compare with gateway
   logs using the event name, never by copying an error string into a label.
3. Check Redis/runtime and database health before restarting workers. A restart
   resets process-local counters but does not clear usage records or the DLQ.
4. Record the incident, alert fingerprint and remediation in the change log.

## Billing enrichment or settlement

```promql
sum by (component, operation) (
  increase(aether_gateway_billing_enrichment_failures_total[10m])
)
sum by (component, operation) (
  increase(aether_gateway_billing_settlement_failures_total[10m])
)
sum by (component, operation) (
  increase(aether_gateway_billing_video_task_settlement_failures_total[10m])
)
```

Inspect the usage worker and terminal finalizer logs, then verify whether the
event is queued or in the usage DLQ. Do not retry a settlement by hand unless
the adapter's idempotency key is present. Roll back the release only when the
failure began with that release and the dependency is healthy.

## Usage deferred drop

```promql
increase(aether_gateway_usage_runtime_terminal_enqueue_deferred_dropped_total[10m])
increase(aether_gateway_usage_runtime_lifecycle_enqueue_deferred_dropped_total[10m])
```

The SLO for both counters is zero dropped events. This is the loss budget,
not a zero-RPO guarantee for Redis persistence; AOF policy and backup recovery
retain their separate durability limits. Any nonzero observed increase fires
a `critical` alert without an additional hold period; delivery follows the
configured scrape, evaluation and Alertmanager intervals. The operations on-call owns incident coordination and
coordinates the accounting reconciliation; retain `job`, `instance`, `cluster`
and deployment labels with the alert fingerprint.

First restore Redis/runtime availability and confirm the gateway is healthy.
Before restarting or redriving anything, preserve the incident time range,
counter values, restricted receipt and request-id references, and relevant
gateway/Redis logs. Define the pending reconciliation range from the first
observed increment through restored health, then compare that range against the
authoritative persisted usage and settlement records. These counters cannot
recover a missing event payload. Do not automatically create usage records,
settlements, invoices, or other billing entries from the alert; any correction
requires the established accounting reconciliation process and its evidence.

## Insufficient quota

```promql
sum by (component, operation) (
  increase(aether_gateway_billing_insufficient_quota_total[10m])
)
```

This counter means a completed usage settlement was persisted as
`insufficient_quota`. It is not an invoice, a provider-cost measurement, or
authorization to retry, debit a wallet, release a hold, or change historical
records. Inspect the approved account policy and the persisted settlement
record before escalating; recovery or collection requires its separately
approved product and finance policy.

## Usage DLQ

```promql
aether_gateway_usage_queue_dlq_length
aether_gateway_usage_queue_group_pending
aether_gateway_usage_queue_dlq_max_length
aether_gateway_usage_queue_dlq_utilization_per_mille
aether_gateway_usage_queue_dlq_at_retention_boundary
```

Preserve the DLQ. Resolve the dependency or schema issue, run the documented
bounded/idempotent redrive, and verify the length decreases without duplicate
settlements. If poison messages recur, stop redrive and leave the alert active.

The capacity warning fires after five minutes at 80 percent of the configured
DLQ retention threshold. The boundary alert fires on the first scrape at or
above that threshold. Keep job, instance, stream and deployment labels when
routing these alerts; inhibit the warning while the boundary alert fires for
the same series. These are example thresholds to validate against the local
inflow rate and operator response time.

Redis uses approximate MAXLEN trimming, so the raw length may exceed the
threshold while utilization is capped at 1000 per mille. Neither metric counts
lost events. Export the available evidence, restore consumers and use the
bounded redrive procedure; do not delete records merely to clear an alert.
Capacity metrics are absent for disabled/unconfigured queues or failed health
reads. Check `usage_queue_health_unavailable` and scrape health; missing data
must not be treated as an empty or healthy DLQ. These rules do not prove a
production Alertmanager notification was delivered.

## Fail-open

```promql
sum by (operation) (
  increase(aether_gateway_billing_fail_open_total[10m])
)
```

`daily_quota` means the daily usage runtime could not be read and the request
was allowed. `rpm` means the RPM runtime check failed while the configured
policy explicitly permits fail-open. Restore Redis/runtime connectivity first;
do not silence this alert by changing policy in the incident. If traffic must
be drained, use the normal gateway admission controls and record the change.

## Provider 5xx

```promql
sum by (provider) (
  rate(aether_gateway_request_errors_total{status_class="5xx"}[5m])
)
```

Check provider health, key rotation and upstream status before changing routing.
The `provider` label is a bounded provider type such as `openai` or `claude_code`;
unrecognized values map to `unknown`. It is not a configured provider ID.
Never add a model, URL, request ID or raw error label. The RED contract delivered
by Issue #306 counts terminal gateway failures, so inspect the failure origin
before attributing a 5xx spike to upstream availability.

## Repeatable validation

Use Prometheus 3.14.0 `promtool` (the CI workflow pins the release and archive
SHA-256):

```bash
cargo run --locked --quiet -p aether-runtime --example prometheus_billing_fixture > /tmp/aeris-billing.prom
promtool check metrics < /tmp/aeris-billing.prom
promtool check rules docs/operations/prometheus/aether-alerts.yml
promtool test rules docs/operations/prometheus/aether-alerts.test.yml
```

The fixture invokes the real billing recorders and exporter. Rule tests cover
usage/video settlement, enrichment, daily quota/RPM fail-open, DLQ and request
5xx. They verify `for` windows, zero-growth controls, reset handling, recovery
and the labels used for incident routing. The `Prometheus Contracts` workflow
uploads the rendered metrics, tool version and results on every PR.

These checks prove parser and rule behavior. They do not substitute for
deployment scraping, delivery to an external Alertmanager, or fault injection
at every production event point; record those separately when accepting #307.

The required Prometheus CI also runs the real delivery harness with
`python3 tests/metrics_acceptance_harness.py --scenario dlq --evidence-dir
<new-evidence-directory>`. It requires Prometheus 3.14.0, its matching promtool,
and Alertmanager 0.34.0 on PATH (or the harness's explicit binary arguments).
It loads the unchanged checked-in rules, serves synthetic capacity samples on
an authenticated loopback endpoint, and verifies a healthy scrape followed by
the retention-boundary firing and resolved webhooks. Both notifications must
retain `alertname`, `severity`, `job`, `instance` and `stream`. Missing or wrong
scrape credentials are also rejected. It uses only owned loopback listeners;
no production receiver is contacted.

The CI artifact includes the exact rule file and its SHA-256, tool versions,
target status, webhook bodies and the result. Temporary service state is
removed after both processes stop; failure logs remain in the evidence
directory. On 2026-09-17 the DLQ scenario passed locally with those binaries.
This proves local rule-to-webhook delivery. Gateway metric production is
covered separately by the authenticated HTTP queue-health test, and deployment
scraping and delivery to the operator's actual notification destination still
require deployment evidence.

## Silence and recovery

Silence by `alertname`, `component`, `operation` or `provider` only, with an
incident reference and an expiry. Remove the silence after two consecutive
evaluation windows show zero new failures and the backlog/health checks are
normal. A zero counter after restart is not proof of recovery; use dependency
health and logs as corroborating evidence.

## On-call alert index and escalation

Every rule in `docs/operations/prometheus/aether-alerts.yml` maps to one entry
below. Severity comes from the rule's `severity` label. Escalation is to the
operations on-call; the on-call owns incident coordination and engages the
accounting reconciliation or release owner as listed.

| Alert | Severity | Meaning | First action | Escalate when |
| --- | --- | --- | --- | --- |
| AetherBillingEnrichmentFailures | warning | Billing enrichment failed; settlement may be persisted at 0 | Inspect usage worker logs; check queue/DLQ | Any persisted 0-amount settlement or repeat after dependency restore |
| AetherBillingSettlementFailures | warning | Terminal settlement failed | Verify idempotency key before any retry | Manual retry needed, or failure started with a release |
| AetherBillingVideoTaskSettlementFailures | warning | Video task settlement failed | Same as settlement failures | Same as settlement failures |
| AetherBillingInsufficientQuota | warning | Settlement persisted as `insufficient_quota` | Inspect account policy and persisted record | Recovery/collection decision needed (separate finance policy) |
| AetherUsageDeadLetterBacklog | warning | Usage DLQ non-empty | Preserve DLQ; fix dependency/schema | Redrive does not shrink length or poison messages recur |
| AetherUsageDeadLetterCapacityWarning | warning | DLQ above 80% retention for 5m | Plan bounded redrive | Boundary alert fires for the same series |
| AetherUsageDeadLetterRetentionBoundary | critical | DLQ at/above retention threshold | Stop-loss: restore consumers before data ages out | Immediately; this is the loss boundary |
| AetherUsageTerminalDeferredDrop | critical | Terminal deferred enqueue dropped | Treat as usage event loss budget spend | Any nonzero increase; coordinate accounting reconciliation |
| AetherUsageLifecycleDeferredDrop | critical | Lifecycle deferred enqueue dropped | Same as terminal deferred drop | Same as terminal deferred drop |
| AetherBillingGuardFailOpen | critical | Quota/RPM guard allowed traffic during runtime outage | Restore Redis/runtime first | Fail-open persists past one evaluation window |
| AetherProviderServerErrors | warning | Terminal gateway 5xx by provider type | Check provider health/keys/status before rerouting | Failover decision or provider outage |
| AetherAdminAuditPersistenceFailures | warning | Admin audit write failed/timeout | Follow `admin-audit-persistence.md#incident-response` | Committed state unverifiable or writer restore needed |

Alert-to-section cross-reference: enrichment/settlement rows → "Billing
enrichment or settlement"; insufficient quota → "Insufficient quota"; DLQ rows
→ "Usage DLQ"; deferred drop rows → "Usage deferred drop"; fail-open →
"Fail-open"; provider 5xx → "Provider 5xx". The admin audit row defers to its
dedicated runbook because a failed audit write may already be committed and
has its own replay/preservation procedure.
