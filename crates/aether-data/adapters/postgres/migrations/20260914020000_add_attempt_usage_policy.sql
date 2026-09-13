-- Attempt policy is a server-owned capability separate from quote/report JSON.
ALTER TABLE public.request_fund_reservations ADD COLUMN IF NOT EXISTS usage_policy jsonb;
DO $$ BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_constraint
        WHERE conname = 'request_fund_reservations_usage_policy_check'
          AND conrelid = 'public.request_fund_reservations'::regclass
    ) THEN
        ALTER TABLE public.request_fund_reservations ADD CONSTRAINT request_fund_reservations_usage_policy_check
            CHECK (usage_policy IS NULL OR (attempt_id IS NOT NULL AND jsonb_typeof(usage_policy) = 'object'
                AND octet_length(usage_policy::text) <= 32768));
    END IF;
END $$;

-- One quota reservation per operation. Its token is the attempt funds token,
-- while the frozen policy carries the distinct original request context token.
ALTER TABLE public.usage_cost_reservations ADD COLUMN IF NOT EXISTS attempt_reservation_token varchar(128);
DO $$ BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_constraint
        WHERE conname = 'usage_cost_reservations_attempt_token_check'
          AND conrelid = 'public.usage_cost_reservations'::regclass
    ) THEN
        ALTER TABLE public.usage_cost_reservations ADD CONSTRAINT usage_cost_reservations_attempt_token_check
            CHECK (attempt_reservation_token IS NULL OR attempt_reservation_token = reservation_token);
    END IF;
    IF NOT EXISTS (
        SELECT 1
        FROM pg_constraint
        WHERE conname = 'usage_cost_reservations_attempt_token_fkey'
          AND conrelid = 'public.usage_cost_reservations'::regclass
    ) THEN
        ALTER TABLE public.usage_cost_reservations ADD CONSTRAINT usage_cost_reservations_attempt_token_fkey
            FOREIGN KEY (attempt_reservation_token) REFERENCES public.request_fund_reservations(reservation_token)
            ON DELETE RESTRICT;
    END IF;
END $$;
CREATE UNIQUE INDEX IF NOT EXISTS usage_cost_reservations_attempt_token_idx
    ON public.usage_cost_reservations (attempt_reservation_token);
