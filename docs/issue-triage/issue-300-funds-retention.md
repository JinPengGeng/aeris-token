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
- Usage marked `insufficient_quota`, including legacy rows with no recovery row
  yet. Retention is not authority to forgive or silently delete that liability.
- Recovery records whose prior entitlement payments plus collections are below
  their frozen cost, even if the usage's billing-status label is stale.

Released/settled reservations and fully collected debts return to ordinary
record retention. No wallet balance, price, receipt, or settlement is rewritten
by cleanup. The new predicate uses the request-ID indexes already present in
the funds and recovery tables; no schema migration or new periodic job is needed.

Financial retention preserves the usage identity, dimensions and pricing
metadata. It does not grant indefinite retention to raw request/response bodies
or headers. Cleanup deletes eligible records first, then applies the selected
payload retention policies to every surviving row, including old financial
rows. Preview counts use that same projected set of surviving rows. Dedicated
body cleanup keeps its existing policy and manual modes.

The required PostgreSQL harness now includes a lifecycle regression that uses
real reserve/dispatch/release/finalize/recovery APIs and isolated tables copied
from migrated schema. With a batch size of one, it verifies seven financial
rows survive, four resolved rows expire, and the expired bodies/headers of the
retained rows are removed while frozen facts remain. It then actually finalizes
one retained reservation and finishes a partial debt collection; a later cleanup
deletes those two resolved rows, and a repeat deletes nothing. Outstanding
wallet holds remain intact throughout.

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
