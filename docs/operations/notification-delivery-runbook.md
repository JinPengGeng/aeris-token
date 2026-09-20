# Notification delivery deployment runbook

This runbook covers the durable refund-status notification worker introduced
for Issue #247. It is a deployment and verification checklist; local SMTP
tests do not establish that a production provider accepts or delivers mail.

## Required topology

Run at least one Gateway instance with the `background` node role against the
same PostgreSQL database used by the admin/API instances. The background role
starts `wallet.refund.notification.worker` when the configured data backend
supports the `refund_status_notifications` repository. Frontdoor-only nodes do
not replace this worker. A multi-node deployment may run the background role on
more than one instance against the shared database: row-level claims and lease
fencing prevent two workers from acknowledging the same event. Start with one
background instance for rollout, then add another only after observing queue
latency and database capacity.

Apply the schema migration before starting the worker. The
`refund_status_notifications` table is the source of truth; do not replay
completed refunds or send mail directly from the admin completion/failure
request.

## SMTP and notification configuration

Configure these system settings through the admin system configuration API/UI
without putting secrets in environment files or logs:

- `module.important_notification.enabled=true`
- `module.important_notification.email_enabled=true`
- `module.important_notification.items` containing an enabled
  `user_refund_status` item with `channel=email` and `user_email_enabled=true`
- `smtp_host`, `smtp_port` (default 587), `smtp_user`, `smtp_password`, and the
  matching TLS/SSL mode

The recipient must have an active account, a non-empty email address, and
`email_notifications=true`. Missing preferences retain the historical enabled
default; a preference-store error skips delivery and leaves the durable event
for retry. `usage_alerts` controls low-balance notices and is not required for
refund status mail. `NOTIFICATION_EMAIL_AVAILABLE` may be used by deployment
health reporting, but it does not configure SMTP or make delivery succeed.

## Post-deploy smoke test

1. Confirm `/readyz` is healthy on the background instance and inspect the
   task-runtime status for `wallet.refund.notification.worker`.
2. Use the admin notification test route with an explicit item and email
   channel (`POST /api/admin/system/important-notification/test`) to verify
   SMTP connectivity. Treat a successful response as provider acceptance for
   that test message only.
3. In a disposable account, complete or fail one refund and verify one row is
   inserted into `refund_status_notifications` and then reaches `delivered`.
   Check the stable event id (`refund:<refund-id>:<status>`), recipient, and
   absence of administrative failure diagnostics in the received body.
4. Stop the background process after claim, restart it after the five-minute
   lease expires, and verify the event is retried without a second refund or
   wallet mutation. A lost SMTP acknowledgement can result in at-least-once
   email delivery.

## Monitoring and recovery

Alert on `state in ('retry','manual_review')`, an expired lease that remains
pending, and sustained growth of `next_attempt_at <= now()` rows. Delivery
failures back off at 1m/5m/30m/2h/6h/24h; the seventh failed attempt enters
`manual_review`. A `skipped` event is revisited daily after configuration or
consent changes. Resolve SMTP/configuration issues, then redrive the event
through the normal worker path; never call the refund provider again to repair
notification delivery.

For non-PostgreSQL backends, this durable worker is unavailable and the legacy
best-effort handler remains subject to that backend's delivery behavior. Do
not claim durable refund notification guarantees until the PostgreSQL worker
and its database are deployed.
