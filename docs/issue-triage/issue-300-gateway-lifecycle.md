# Issue 300 gateway lifecycle decision

Status: accepted implementation scope; Gateway integration remains in progress.
Refs #300 and #206. This is a decision record, not runtime delivery evidence.

PR #362 merged as `ab1dfa9686e6e4937fee89fdbc48a04b876e57ee` on 2026-09-13.
The fork main baseline `3d66f7c30729347d4d8dc23f9802469784bf7714` contains its
reserve/dispatch/release/finalize and recovery interfaces, but has no Gateway
call sites using them. The existing `plan_usage_policy` reservation is a
separate, expiring usage-limit ledger; it cannot represent wallet or entitlement
holds. The data API being present does not prove a complete financial lifecycle.

## Compatibility boundary

Each possibly billable upstream attempt needs its own server-issued identity,
reservation token, frozen provider/key/model and non-secret quote. Unknown
dispatched work retains its hold. A retry that can itself incur a charge must
obtain new authorization; replaying the same attempt's terminal event is the
case that reuses its identity and remains idempotent.

Keep one external `usage.request_id`. Extend the reservation as an attempt
financial child record and aggregate known actual cost, collected amount,
remaining hold and unknown outcomes into the parent. Do not manufacture usage
request IDs to evade the current single-request settlement uniqueness.
The existing v1 contract is insufficient to represent several independently
charged attempts and must remain restricted to its legacy records.

Sync, stream, heartbeat and format-conversion paths must use the same durable
financial event contract. A known charge consumes only its authorized amount;
excess actual cost remains reconciliation evidence. Proven no-charge work may
release its hold. Cancellation, timeout and an unparseable upstream response
are not proof of zero cost. Unknown work stays pending and may accept a later
authoritative outcome without a second charge.

Ordinary usage upserts, settlement and recharge recovery must respect a durable
parent billing mode so that old or token-less events cannot overwrite the
aggregate or debit it again. Keep per-attempt provider attribution in both
online deltas and statistics rebuilds, with one external request count.

## Dependency decision

The initial #362 merge dependency is resolved. Implementation now requires
attempt child accounting, parent financial fencing/aggregation, Gateway
admission/dispatch, and both queued and direct usage writers. Confirm the parent
usage row is persisted before the first reserve; best-effort pending writes
are insufficient for a financial prerequisite. Use the established lock order:
request advisory lock, usage row, wallet, entitlements, then reservation.

PR #388 addresses unresolved-financial-record retention, including
snapshot-first debt state and cleanup/settlement concurrency. It remains a
separate prerequisite; retention cannot substitute for attempt integration.
Prepared recovery must race dispatch through a conditional database transition;
dispatched unknown holds cannot expire solely on elapsed time.

Required acceptance includes a USD 0.10 wallet facing two USD 0.08 attempts,
where only one attempt reaches the upstream; and a USD 0.20 wallet where A
holds USD 0.08 with unknown cost, B independently settles USD 0.06, and A later
settles USD 0.07. The latter must leave one external usage row, USD 0.13 total
collected, correct attribution to both providers, and no extra debit after
outbox cleanup and replay. Entitlement, API key-owned/standalone, unlimited,
partial-output and legacy event paths also need explicit coverage.

Do not enable unknown paid-image requests until the complete quote and Gateway
funding acceptance pass review and required checks. #300 and #206 remain open;
the earlier blocked status referred only to #362 and is superseded by this
record. Detailed data implementation decisions belong in the attempt-funds
implementation PR, with tests and migration compatibility evidence.
