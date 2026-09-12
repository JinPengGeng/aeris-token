CREATE TABLE IF NOT EXISTS public.request_fund_reservations (
    reservation_token varchar(128) PRIMARY KEY,
    request_id varchar(128) NOT NULL UNIQUE,
    wallet_id varchar(64) REFERENCES public.wallets(id) ON DELETE RESTRICT,
    quote jsonb NOT NULL,
    state varchar(32) NOT NULL CHECK (state IN ('prepared', 'dispatched', 'settled', 'released', 'reconciliation_pending')),
    actual_cost_units bigint CHECK (actual_cost_units >= 0 AND actual_cost_units <= 4503599627370495),
    collected_cost_units bigint NOT NULL DEFAULT 0 CHECK (collected_cost_units >= 0 AND collected_cost_units <= 4503599627370495),
    reconciliation_facts jsonb,
    settlement jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS request_fund_reservations_wallet_state_idx
    ON public.request_fund_reservations (wallet_id, state);

CREATE TABLE IF NOT EXISTS public.request_fund_allocations (
    reservation_token varchar(128) NOT NULL REFERENCES public.request_fund_reservations(reservation_token) ON DELETE RESTRICT,
    ordinal integer NOT NULL CHECK (ordinal >= 0),
    source_kind varchar(24) NOT NULL CHECK (source_kind IN ('wallet_recharge', 'wallet_gift', 'entitlement', 'postpaid')),
    source_id varchar(128) NOT NULL,
    usage_date varchar(10),
    quota_cost_units bigint CHECK (quota_cost_units >= 0 AND quota_cost_units <= 4503599627370495),
    reserved_cost_units bigint NOT NULL CHECK (reserved_cost_units >= 0 AND reserved_cost_units <= 4503599627370495),
    collected_cost_units bigint NOT NULL DEFAULT 0 CHECK (collected_cost_units >= 0 AND collected_cost_units <= reserved_cost_units),
    PRIMARY KEY (reservation_token, ordinal),
    CHECK ((source_kind = 'entitlement') = (usage_date IS NOT NULL)),
    CHECK ((source_kind = 'entitlement') = (quota_cost_units IS NOT NULL))
);
CREATE INDEX IF NOT EXISTS request_fund_allocations_source_idx
    ON public.request_fund_allocations (source_id, source_kind, usage_date);

CREATE TABLE IF NOT EXISTS public.request_fund_recoveries (
    request_id varchar(128) PRIMARY KEY,
    wallet_id varchar(64) NOT NULL REFERENCES public.wallets(id) ON DELETE RESTRICT,
    frozen_actual_cost_units bigint NOT NULL CHECK (frozen_actual_cost_units >= 0 AND frozen_actual_cost_units <= 4503599627370495),
    prior_entitlement_cost_units bigint NOT NULL CHECK (prior_entitlement_cost_units >= 0),
    collected_cost_units bigint NOT NULL DEFAULT 0 CHECK (collected_cost_units >= 0),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (prior_entitlement_cost_units + collected_cost_units <= frozen_actual_cost_units)
);

CREATE TABLE IF NOT EXISTS public.request_fund_collection_receipts (
    id varchar(64) PRIMARY KEY,
    request_id varchar(128) NOT NULL REFERENCES public.request_fund_recoveries(request_id) ON DELETE RESTRICT,
    collected_cost_units bigint NOT NULL CHECK (collected_cost_units > 0),
    recharge_before double precision NOT NULL,
    recharge_after double precision NOT NULL,
    gift_before double precision NOT NULL,
    gift_after double precision NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS request_fund_collection_receipts_request_idx
    ON public.request_fund_collection_receipts (request_id);
