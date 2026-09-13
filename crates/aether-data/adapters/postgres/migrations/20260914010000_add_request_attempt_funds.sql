-- Keep one public usage row while retries own separate, durable financial attempts.
-- Existing rows remain legacy: this migration neither reprices nor replays them.
ALTER TABLE public.usage
    ADD COLUMN IF NOT EXISTS billing_mode varchar(20) NOT NULL DEFAULT 'legacy',
    ADD COLUMN IF NOT EXISTS funds_admission_closed_at timestamptz,
    DROP CONSTRAINT IF EXISTS usage_billing_mode_check,
    ADD CONSTRAINT usage_billing_mode_check
        CHECK (billing_mode IN ('legacy', 'attempt_funds'));

ALTER TABLE public.request_fund_reservations
    ADD COLUMN IF NOT EXISTS attempt_id uuid,
    ADD COLUMN IF NOT EXISTS candidate_id varchar(128),
    ADD COLUMN IF NOT EXISTS provider_id varchar(128),
    ADD COLUMN IF NOT EXISTS provider_api_key_id varchar(128),
    ADD COLUMN IF NOT EXISTS model_id varchar(128),
    ADD COLUMN IF NOT EXISTS dispatched_at timestamptz,
    ADD COLUMN IF NOT EXISTS terminal_facts jsonb,
    DROP CONSTRAINT IF EXISTS request_fund_reservations_attempt_provider_check,
    ADD CONSTRAINT request_fund_reservations_attempt_provider_check
        CHECK (attempt_id IS NULL OR provider_id IS NOT NULL),
    DROP CONSTRAINT IF EXISTS request_fund_reservations_terminal_facts_check,
    ADD CONSTRAINT request_fund_reservations_terminal_facts_check
        CHECK (terminal_facts IS NULL OR (
            jsonb_typeof(terminal_facts) = 'object'
            AND terminal_facts -> 'schema_version' = '1'::jsonb
            AND terminal_facts ? 'schema_version'
            AND octet_length(terminal_facts::text) <= 20480
        )),
    DROP CONSTRAINT IF EXISTS request_fund_reservations_request_id_key;

CREATE UNIQUE INDEX IF NOT EXISTS request_fund_reservations_legacy_request_idx
    ON public.request_fund_reservations (request_id)
    WHERE attempt_id IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS request_fund_reservations_attempt_id_idx
    ON public.request_fund_reservations (attempt_id)
    WHERE attempt_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS request_fund_reservations_request_id_idx
    ON public.request_fund_reservations (request_id);

ALTER TABLE public.entitlement_usage_ledgers
    ADD COLUMN IF NOT EXISTS attempt_id uuid,
    DROP CONSTRAINT IF EXISTS uq_entitlement_usage_request;
-- Legacy writers must use ON CONFLICT (...) WHERE attempt_id IS NULL.
CREATE UNIQUE INDEX IF NOT EXISTS uq_entitlement_usage_request
    ON public.entitlement_usage_ledgers (user_entitlement_id, request_id)
    WHERE attempt_id IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS uq_entitlement_usage_attempt
    ON public.entitlement_usage_ledgers (user_entitlement_id, request_id, attempt_id)
    WHERE attempt_id IS NOT NULL;

ALTER TABLE public.usage_settlement_snapshots
    ADD COLUMN IF NOT EXISTS request_funds_summary jsonb;
