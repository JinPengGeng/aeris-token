-- No historical backfill: only new principal-credit inserts observed by the
-- deferred trigger while this durable activation is enabled authorize collection.
CREATE TABLE IF NOT EXISTS public.recharge_recovery_activation (
    version integer PRIMARY KEY,
    enabled boolean NOT NULL,
    activated_at timestamptz NOT NULL
);
INSERT INTO public.recharge_recovery_activation (version, enabled, activated_at)
VALUES (1, true, clock_timestamp()) ON CONFLICT (version) DO NOTHING;

CREATE TABLE IF NOT EXISTS public.recharge_recovery_jobs (
    id varchar(64) PRIMARY KEY,
    payment_order_id varchar(64) NOT NULL UNIQUE REFERENCES public.payment_orders(id),
    source_transaction_id varchar(64) NOT NULL UNIQUE REFERENCES public.wallet_transactions(id),
    wallet_id varchar(64) NOT NULL REFERENCES public.wallets(id),
    user_id varchar(64),
    activation_version integer NOT NULL REFERENCES public.recharge_recovery_activation(version),
    principal_cost_units bigint NOT NULL CHECK (principal_cost_units > 0 AND principal_cost_units <= 4503599627370495),
    collected_cost_units bigint NOT NULL DEFAULT 0 CHECK (collected_cost_units >= 0),
    outstanding_cost_units bigint NOT NULL DEFAULT 0 CHECK (outstanding_cost_units >= 0),
    available_recharge_cost_units bigint NOT NULL DEFAULT 0 CHECK (available_recharge_cost_units >= 0),
    state text NOT NULL DEFAULT 'pending' CHECK (state IN ('pending','retry','completed','waiting_next_recharge','manual_review','source_unavailable')),
    operation_seq bigint NOT NULL DEFAULT 0,
    retry_count integer NOT NULL DEFAULT 0 CHECK (retry_count >= 0),
    next_attempt_at timestamptz DEFAULT now(),
    error_code text,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (collected_cost_units <= principal_cost_units)
);
CREATE INDEX IF NOT EXISTS recharge_recovery_jobs_due_idx ON public.recharge_recovery_jobs(next_attempt_at, created_at, id) WHERE state IN ('pending','retry');
CREATE INDEX IF NOT EXISTS recharge_recovery_jobs_owner_idx ON public.recharge_recovery_jobs(user_id, created_at);

-- Candidate membership is an immutable authorization snapshot. Usage timestamps
-- are only FIFO metadata; later failures or backdated inserts cannot join a budget.
CREATE TABLE IF NOT EXISTS public.recharge_recovery_candidates (
    job_id varchar(64) NOT NULL REFERENCES public.recharge_recovery_jobs(id),
    request_id varchar(128) NOT NULL,
    PRIMARY KEY (job_id, request_id)
);

CREATE TABLE IF NOT EXISTS public.recharge_recovery_operations (
    job_id varchar(64) NOT NULL REFERENCES public.recharge_recovery_jobs(id),
    operation_seq bigint NOT NULL,
    request_id varchar(128) NOT NULL,
    receipt_id varchar(64) NOT NULL UNIQUE REFERENCES public.request_fund_collection_receipts(id),
    wallet_transaction_id varchar(64) NOT NULL UNIQUE REFERENCES public.wallet_transactions(id),
    collected_cost_units bigint NOT NULL CHECK (collected_cost_units > 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (job_id, operation_seq)
);
CREATE TABLE IF NOT EXISTS public.recharge_recovery_notifications (
    id varchar(64) PRIMARY KEY,
    job_id varchar(64) NOT NULL REFERENCES public.recharge_recovery_jobs(id),
    audience text NOT NULL CHECK (audience IN ('user','admin')),
    summary jsonb NOT NULL,
    state text NOT NULL DEFAULT 'pending' CHECK (state IN ('pending','retry','skipped','delivered','manual_review')),
    attempts integer NOT NULL DEFAULT 0,
    lease_token bigint NOT NULL DEFAULT 0,
    lease_until timestamptz,
    next_attempt_at timestamptz NOT NULL DEFAULT now(),
    error_code text,
    delivered_at timestamptz,
    UNIQUE (job_id, audience)
);
CREATE INDEX IF NOT EXISTS recharge_recovery_notifications_due_idx ON public.recharge_recovery_notifications(next_attempt_at) WHERE state IN ('pending','retry','skipped');

CREATE OR REPLACE FUNCTION public.enqueue_recharge_debt_recovery() RETURNS trigger
LANGUAGE plpgsql SET search_path = public, pg_temp AS $$
DECLARE
    activation record;
    payment record;
    units numeric;
    enqueued_job_id varchar(64);
BEGIN
    -- Transaction-local import marker remains set through deferred trigger execution.
    -- It never disables collection for concurrently committed live payments.
    IF current_setting('aether.recharge_recovery_restore', true) = 'on' THEN
        RETURN NEW;
    END IF;
    IF NEW.category IS DISTINCT FROM 'recharge' OR NEW.reason_code IS NULL
       OR NEW.reason_code NOT IN ('topup_gateway','topup_admin_manual','topup_card_code')
       OR NEW.link_type IS DISTINCT FROM 'payment_order' OR NEW.link_id IS NULL
       OR NEW.amount IS NULL OR NEW.amount <= 0
       OR NEW.recharge_balance_before IS NULL OR NEW.recharge_balance_after IS NULL
       OR NEW.recharge_balance_after <= NEW.recharge_balance_before
       OR NEW.gift_balance_after IS DISTINCT FROM NEW.gift_balance_before THEN
        RETURN NEW;
    END IF;
    SELECT * INTO activation FROM recharge_recovery_activation WHERE version = 1 AND enabled;
    -- NOW()/receipt.created_at is transaction start, not the credit boundary.
    -- A transaction begun before activation may still commit a new credit after it.
    IF NOT FOUND THEN RETURN NEW; END IF;
    SELECT p.*, CASE WHEN w.api_key_id IS NULL THEN w.user_id ELSE k.user_id END AS wallet_owner INTO payment
    FROM payment_orders p JOIN wallets w ON w.id = p.wallet_id
    LEFT JOIN api_keys k ON k.id = w.api_key_id
    WHERE p.id = NEW.link_id AND p.wallet_id = NEW.wallet_id
      AND p.order_kind = 'wallet_recharge' AND p.status = 'credited'
      AND p.credited_at IS NOT NULL AND p.paid_at IS NOT NULL
      AND p.refunded_amount_usd = 0 AND p.amount_usd > 0
      AND (w.api_key_id IS NULL OR k.id IS NOT NULL)
      AND (p.user_id IS NULL OR p.user_id IS NOT DISTINCT FROM CASE WHEN w.api_key_id IS NULL THEN w.user_id ELSE k.user_id END);
    IF NOT FOUND THEN RETURN NEW; END IF;
    units := FLOOR(LEAST(NEW.amount::numeric, payment.amount_usd::numeric,
                        NEW.recharge_balance_after::numeric - NEW.recharge_balance_before::numeric) * 100000000);
    IF units <= 0 OR units > 4503599627370495 THEN RETURN NEW; END IF;
    INSERT INTO recharge_recovery_jobs
        (id, payment_order_id, source_transaction_id, wallet_id, user_id, activation_version, principal_cost_units)
    VALUES (NEW.id, payment.id, NEW.id, NEW.wallet_id, payment.wallet_owner, activation.version, units::bigint)
    ON CONFLICT (payment_order_id) DO NOTHING RETURNING id INTO enqueued_job_id;
    IF enqueued_job_id IS NULL THEN RETURN NEW; END IF;
    INSERT INTO recharge_recovery_candidates (job_id, request_id)
    SELECT enqueued_job_id, u.request_id
    FROM usage u
    LEFT JOIN usage_settlement_snapshots s ON s.request_id = u.request_id
    JOIN wallets w ON w.id = NEW.wallet_id
    WHERE COALESCE(s.billing_status, u.billing_status) = 'insufficient_quota'
      AND u.billing_mode = 'legacy'
      AND u.user_id IS NOT DISTINCT FROM payment.wallet_owner
      AND ((w.api_key_id IS NOT NULL AND u.api_key_id = w.api_key_id)
        OR (w.api_key_id IS NULL AND w.user_id = u.user_id
          AND NOT EXISTS (SELECT 1 FROM wallets kw WHERE kw.api_key_id = u.api_key_id)));
    RETURN NEW;
END;
$$;
DROP TRIGGER IF EXISTS enqueue_recharge_debt_recovery ON public.wallet_transactions;
-- Payment callbacks write the credit receipt before marking the order credited.
-- A deferred trigger sees the final order state and commits the budget and visible
-- debt membership atomically. Trigger installation locks this table and waits for
-- preceding writers; existing committed receipts are never scanned/backfilled.
-- Restore paths must suppress enqueue while reinserting historical receipts.
CREATE CONSTRAINT TRIGGER enqueue_recharge_debt_recovery
AFTER INSERT ON public.wallet_transactions DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW EXECUTE FUNCTION public.enqueue_recharge_debt_recovery();
