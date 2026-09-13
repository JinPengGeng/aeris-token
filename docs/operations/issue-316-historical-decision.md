# Issue #316 historical rebate decision

Decision date: 2026-09-13

## Evidence reviewed

- `origin/main` at `c860eec66d9edfb39f7536111bd7e8f6d1b2ca66` contains merged PR
  #325 (`c58ccc858e74b785686ef8857d6fc7c3d0b0337a`).
- PR #325 adds the read-only, repeatable-read aggregate audit and a PostgreSQL
  NUMERIC cast regression fixture. Its isolated PostgreSQL test passed.
- No production snapshot, read-replica export, or redacted aggregate output for
  `referral_rewards` and `wallet_transactions` is present in the repository or
  in the Issue #316 record reviewed on this date.

## Decision

Historical impact remains **unknown**. No backfill is required or authorized at
this time because there is no production evidence from which to determine
whether a repair is needed. This is a fork-only decision record; it does not
write production data and does not close Issue #316 or parent Issue #208.

Do not mark the issue complete based on the merged toolkit alone. A future
operator may record “no repair required” only after running the exact audit from
`referral-rebate-numeric-audit.sql` against a consistent production snapshot,
including all four aggregate result sets and the snapshot timestamp/SHA. Any
non-zero missing link, ledger identity mismatch, signed amount difference, or
malformed counter requires a separately reviewed, idempotent repair proposal.

## Project status suggestion

Keep the item in `Planned` (or move to `In Progress` when an operator has
scheduled the snapshot audit). Keep `priority:P2`, `area:data`, and medium risk.
Do not move it to `Done` until the redacted production aggregates and a
reviewed repair/no-repair decision are attached to Issue #316.
