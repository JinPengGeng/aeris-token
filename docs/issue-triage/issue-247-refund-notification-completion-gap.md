# Issue #247: completion notification follow-up

Snapshot: 2026-09-13, fork `JinPengGeng/aeris-token`.

The existing refund notification slice wired the `fail` mutation, but the
`processing -> succeeded` completion handler did not invoke the shared user
notification dispatcher. This left successful refunds silent even though the
default `user_refund_status` item and SMTP path were configured.

This follow-up invokes the dispatcher after a successful completion mutation,
using the pre-mutation status to preserve the existing terminal-transition
guard. Delivery remains best-effort and cannot roll back the durable refund;
terminal retries are not notified. No changes are made to low-balance
threshold producers or provider quota alert policy in this slice.

Refs #247.
