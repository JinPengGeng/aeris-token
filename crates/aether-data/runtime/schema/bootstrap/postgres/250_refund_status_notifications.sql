-- No backfill: only future repository terminal transitions enqueue notifications.
-- This table conveys delivery obligations only; it never authorizes money movement.
CREATE TABLE IF NOT EXISTS public.refund_status_notifications (
    id varchar(128) PRIMARY KEY,
    refund_id varchar(64) NOT NULL UNIQUE REFERENCES public.refund_requests(id),
    wallet_id varchar(64) NOT NULL REFERENCES public.wallets(id),
    user_id varchar(64),
    refund_no varchar(64) NOT NULL,
    amount_usd numeric(20,8) NOT NULL CHECK(amount_usd > 0),
    terminal_status text NOT NULL CHECK(terminal_status IN ('succeeded','failed')),
    failure_reason varchar(500),
    state text NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','retry','skipped','delivered','manual_review')),
    attempts integer NOT NULL DEFAULT 0 CHECK(attempts >= 0),
    lease_token bigint NOT NULL DEFAULT 0,
    lease_until timestamptz,
    next_attempt_at timestamptz NOT NULL DEFAULT now(),
    error_code text,
    delivered_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS refund_status_notifications_due_idx
ON public.refund_status_notifications(next_attempt_at,id)
WHERE state IN ('pending','retry','skipped');
