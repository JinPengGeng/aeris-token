# Issue #247: refund status notification slice

Updated: 2026-09-17, fork `JinPengGeng/aeris-token`.

## Decision

Issue #247 identifies a real product gap: refund completion and failure are
durable state transitions. The original email calls could be lost between the
business commit and delivery. The current slice adds transactional notification
delivery for these terminal transitions:

- `processing -> succeeded` (`/api/admin/wallets/{wallet}/refunds/{refund}/complete`)
- `pending_approval|approved|processing -> failed` (`.../fail`)

Each transition uses the existing user-email notification dispatcher and the
new `user_refund_status` item. The item is enabled by default, scoped to email,
and requires the existing `user_email_enabled` item setting plus the recipient's
`email_notifications` preference. `usage_alerts` remains scoped to low-balance
alerts. Missing preferences retain the historical enabled default;
preference-store failures fail closed.
No email is sent for API-key or orphaned wallets, users without an address,
disabled items, opted-out recipients, or missing SMTP configuration.

## Reliability and privacy

PostgreSQL commits the terminal refund and one unique notification event in
the same transaction. An enqueue failure rolls back that local transition;
previously committed provider refund evidence remains available for safe
recovery. No historical terminal refunds are automatically backfilled.

A separate worker claims one event immediately before delivery using a
five-minute lease and increasing token. Database time and token fencing reject
expired/replayed ACKs. Technical failures retry after 1m/5m/30m/2h/6h/24h;
the seventh failure enters manual review. Disabled consent/configuration is
rechecked daily without consuming failed-delivery attempts. Notification work
never retries the provider refund or changes wallet balances.

The PostgreSQL handler no longer sends email directly after committing. Other
backends retain their existing best-effort handler notification until they have
an equivalent durable repository. A capability check also prevents that fallback
from sending alongside the PostgreSQL worker.

The database guarantees one logical event per terminal refund. SMTP success
followed by a lost ACK can still produce another email after retry; external
delivery is not exactly once. Stable refund/event identifiers identify replay.

Templates receive the refund number, exact decimal amount, terminal status,
stable event identifier and a fixed user-facing failure message. Administrative
failure reasons are free text and can contain internal diagnostics; truncation
alone does not make them suitable for email. Those diagnostics remain in the
refund record, while neither the notification snapshot nor email carries them.
The renderer continues escaping HTML and logs redact dispatcher errors.

## Acceptance evidence

- Default item exists in both the runtime fallback and admin system defaults.
- New exact PostgreSQL tests cover all terminal-branch rollback, concurrent
  completion/replay, lease expiry/fencing, retry budgets and owner/state changes.
- New Gateway PostgreSQL/SMTP tests cover failure, restart, recovery, consent,
  unavailable preferences and internal-diagnostic exclusion. A real loopback
  SMTP fallback test covers non-PostgreSQL delivery and PostgreSQL exclusion.
- `cargo fmt --all -- --check`, `git diff --check`, and the focused gateway
  test suite are required before merge.

The implementation is integrated and accepted locally. The final batch executed
all 62 PostgreSQL and 24 Gateway exact targets successfully, including both
real SMTP tests. Ordinary refund regressions passed 34 tests; both harness
fixtures, strict Gateway/changed-crate Clippy, formatting and diff checks passed.
See `refund-final.summary.json` under the retained task log root.

Real SMTP tests exposed missing details in no-template fallback bodies. Both
worker and legacy fallback now include safe refund details and the fixed
failure explanation, and the original text/plain and text/html assertions pass.
Full native restoration passed with 100 public tables and four nonempty refund
events, preserving retry/lease state and replay without changing money. JSONL
imports do not provide notification continuation. Deployed delivery remains a
separate acceptance step.

## Low-balance and configuration follow-up (2026-09-18)

The low-balance producer is now wired to wallet authentication-snapshot
refreshes. It respects both `email_notifications` and `usage_alerts`, requires
an active finite wallet and an active user with a verified email, and
uses `module.important_notification.user_balance_low_threshold` (default USD
10). The existing admin notification page can edit that threshold. A successful
notification is suppressed until the balance recovers; delivery failures and
configuration skips release that process-local suppression so a later snapshot
refresh can retry. This is evaluated on the next real wallet snapshot refresh,
not inside the settlement transaction, and does not promise durable or
cross-instance deduplication.

`provider_pool_abnormal` had no producer. It is removed from new defaults;
existing stored entries display “未接入”, cannot be enabled in the admin page,
and are excluded from its test-delivery selector. This closes the original
configuration illusion without claiming an implemented pool-health notifier.

Frontend type checking passed. Focused Rust runtime verification is tracked in
`followup-acceptance-20260918.md`; source formatting alone is not delivery
acceptance. Public model-error classification is tracked with #254, and
production notification delivery remains a deployment acceptance item.
