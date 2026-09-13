# Retain unresolved request funds evidence

Refs #300, #206 and #223. Decision: accepted, P1, medium implementation scope.
Baseline: fork main `1087c867e08aa517b28a5acfaf162c16ebf298e4`, including #362.

The record-retention job previously selected every usage row older than the log
cutoff. Request-funds finalization and insufficient-quota recovery require that
row's identity and frozen cost evidence. Deleting it could make an outstanding
hold or debt impossible to reconcile. A real PostgreSQL fixture reproduced the
problem: the preview offered all eleven old rows for deletion, including seven
that still had financial obligations; only four were actually eligible.

The record preview and deletion now share a predicate that retains:

- Prepared or dispatched reservations and reconciliation-pending reservations,
  including capped settlements whose debit succeeded but discrepancy is open.
- Usage whose authoritative settlement snapshot is `insufficient_quota`, with
  the legacy usage column used only when no snapshot exists. This includes
  debts with no recovery row yet. Retention is not authority to forgive debt.
- Recovery records whose prior entitlement payments plus collections are below
  their frozen cost, even if the usage's billing-status label is stale.

Released/settled reservations and fully collected debts return to ordinary
record retention. No wallet balance, price, receipt, or settlement is rewritten
by cleanup. The new predicate uses the request-ID indexes already present in
the funds and recovery tables; no schema migration or new periodic job is needed.

Each deletion batch locks eligible usage rows with `FOR UPDATE SKIP LOCKED`,
then rechecks the financial predicate in a second statement before deleting.
Settlement and recovery already take the usage row lock before writing debt.
Cleanup therefore skips their in-flight rows and cannot wait on a stale
candidate ID and then delete the newly committed liability. A later cleanup
can revisit skipped records; concurrent preview counts remain advisory.

Financial retention preserves the usage identity, dimensions and pricing
metadata. It does not grant indefinite retention to raw request/response bodies
or headers. Cleanup deletes eligible records first, then applies the selected
payload retention policies to every surviving row, including old financial
rows. Preview counts use that same projected set of surviving rows. Dedicated
body cleanup keeps its existing policy and manual modes.

The required PostgreSQL harness now includes a lifecycle regression that uses
real reserve/dispatch/release/finalize/recovery APIs and isolated tables copied
from migrated schema. With a batch size of one, it verifies eight financial
rows survive, five resolved rows expire, and the expired bodies/headers of the
retained rows are removed while frozen facts remain. It then actually finalizes
one retained reservation, finishes a partial debt collection and recovers a
snapshot-only debt; a later cleanup deletes those three resolved rows, and a
repeat deletes nothing. The fixture explicitly recreates the production
snapshot foreign key, verifying resolved snapshot cascade while unpaid
snapshots survive. Outstanding wallet holds remain intact throughout.

Independent review identified two additional bugs and both were reproduced
before their fixes: the old mirror predicate deleted `snapshot-debt` while
retaining `snapshot-settled`, and a cleanup statement waiting behind a real
PostgreSQL row lock deleted one newly committed liability. The regression now
covers both snapshot disagreement directions and that deterministic two-session
lock barrier. It requires zero deletions of the new debt on cleanup and replay.
The payload assertions use inline body/header capture; they do not claim a new
end-to-end external blob-store test.

The new test failed before the fix and passes after it on disposable PostgreSQL
17.11 with Rust 1.95 (one executed test, zero ignored). Existing payload-retention
regressions and current-head CI supply the remaining validation recorded on the
PR. The fixture uses synthetic data and does not audit or repair production.

This clears the usage-retention prerequisite for Gateway funds integration.
Per-attempt funding identity, real dispatch ownership, authoritative actual-cost
facts, crash/queue reconciliation and recharge-worker scheduling remain required
by #300/#206; no complete Gateway lifecycle is claimed here. Resolve a financial
obligation using its reviewed accounting flow, not by forcing retention to
delete it. Rollback of the application code would restore the old deletion
behavior, so pause record cleanup before such a rollback while obligations remain.
