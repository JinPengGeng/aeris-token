# Administrator audit persistence incidents

Administrator business changes can commit before their audit INSERT. The
gateway awaits that INSERT for at most two seconds and preserves the original
business response on a database error or timeout. A timeout does not establish
that PostgreSQL rolled back: the INSERT may still commit after the gateway
stops awaiting it. Successful inserts are durable; failure delivery currently
has no automatic retry, outbox or reconciliation worker.

This runbook implements the failure-observability part of
[the administrator audit design](../issue-triage/admin-audit-design.md#写失败行为).
Metrics and alerts expose missing or uncertain writes; they neither store the
audit record nor repair its absence. Parent #255 remains open for its remaining
delivery and coverage requirements.

## Signals and alert policy

The default namespace is `aether_gateway` and the default metric label is
`component="gateway"`. Adapt queries when a deployment changes the namespace.
`job`, `instance` and deployment labels come from Prometheus scrape configuration.
Never add user, request, event ID, path, token, raw error or other secret values
as metric or alert labels.

| Metric suffix | Meaning |
| --- | --- |
| `admin_audit_persist_attempts_total` | One increment per audit persistence call; scrapes do not increment it. |
| `admin_audit_persist_failures_total` | Audit persistence errors, including timeouts. |
| `admin_audit_persist_timeouts_total` | The timeout subset of failures; do not add it to failures. |
| `durable_admin_audit_available` | Whether the gateway has a durable audit writer configured. This is not a database health probe or proof of a successful write. |

`AetherAdminAuditPersistenceFailures` fires when
`increase(aether_gateway_admin_audit_persist_failures_total[10m]) > 0`.
There is no minimum attempt count, ratio threshold or `for` delay: a single
failed write must remain visible even if no administrator makes another
request. The ten-minute observation window keeps one stable alert per metric
series; unchanged scrapes do not create new counter events or reset its
activation time. All scrape labels remain available to locate the affected
replica. A timeout produces this same failure alert rather than a second alert.

Route it as a warning with Alertmanager grouping by `alertname`, `component`,
`job`, `instance` and the deployment's cluster labels. Set an incident-appropriate
`group_wait` and `repeat_interval` (for example 30 seconds and four hours), and
enable resolved notifications. Notification deduplication belongs to
Alertmanager; rule tests do not prove delivery to the deployed receiver.

The rule resolves after the observed increment leaves the ten-minute window.
This means no recent observed failures, **not** that a missing audit row has been
recovered. `increase` extrapolates between samples; its value is not an exact
audit event count. A counter without a baseline scrape, an event followed by a
process crash between scrapes, scrape outages or counter resets can hide events.
Keep the deployment's target-down alerts and incident logs; neither a zero
counter nor absence of this alert proves durable audit completeness.

No generic `durable_admin_audit_available == 0` alert is included: a database-free
development instance intentionally has no durable writer. Deployments requiring
durable audit should explicitly enforce writer configuration and monitor this
gauge for their production targets. A value of one can coexist with database
errors and must not suppress the failure alert.

## Incident response

1. Record the affected `job`, `instance`, cluster, UTC interval and alert
   fingerprint. Preserve the process logs before restarting or rolling back.
   Find `admin_audit_persist_failed` or `admin_audit_persist_timeout` events in
   that interval and record each `audit_event_id` in the access-controlled
   incident record. Keep IDs and log details out of metric labels and public
   issue bodies.
2. Check whether the original administrator operation already changed business
   state. **Do not blindly replay the HTTP mutation**: changing a response to an
   error or rerunning a successful request can duplicate an already-committed
   side effect. Preserve the original outcome and inspect the relevant store.
3. Query the audit database for the exact event ID using a read-only session and
   a bound parameter; for example:

   ```sql
   SELECT id, request_id, event_type, status_code, created_at
   FROM audit_logs
   WHERE id = $1;
   ```

   The protected administrator audit API is also useful for investigation, but
   sensitive reads can themselves generate audit writes. During a writer outage,
   a read-only database session avoids producing further failed audit events.
   An absent row after a timeout is provisional while the original INSERT is
   still running; check database activity and recheck after it finishes.
4. Inspect database reachability, credentials, pool exhaustion, INSERT grants,
   migration state and blocking locks. `durable_admin_audit_available = 1`
   confirms wiring only. Restore the failed dependency; do not restart merely to
   make counters zero. For release-correlated failures, preserve the evidence
   and confirm schema compatibility before rolling back.
5. After restoring the dependency, carry out one explicitly intended,
   low-impact administrator action and confirm its business outcome and matching
   audit row. Watch the same instance for new failures across the observation
   window. Record recovered late commits separately from rows still missing.
6. There is no supported automatic replay queue or backfill command for these
   failures. Same-ID repository writes are idempotent, but a diagnostic log is not
   a complete durable copy of the original audit payload. Do not synthesize
   missing events from partial logs or manufacture a new ID and claim recovery.
   Keep any confirmed audit gap in the incident record; an eventual approved
   repair must preserve the original event identity and verified business state.

## Validation

Using the existing Prometheus 3.14.0 toolchain:

```bash
promtool check rules docs/operations/prometheus/aether-alerts.yml
promtool test rules docs/operations/prometheus/aether-alerts.test.yml
```

The tests cover successful and idle instances, one failed write without further
requests, one timeout, unchanged scrapes retaining the same activation,
resolution without inventing recovery, counter resets, and per-job/per-instance
isolation with deployment routing labels. Gateway tests separately establish
the runtime counter increments and unchanged business response. Deployment
scraping and notification delivery must still be validated in the environment
using these rules.
