-- Fork-owned additive table (Refs #526): provider cost catalogs isomorphic to
-- the sales-side BillingModelPricingSnapshot pricing catalog. Rollback:
-- DROP TABLE public.provider_costs;
CREATE TABLE IF NOT EXISTS public.provider_costs (
    cost_id varchar(128) PRIMARY KEY,
    provider_id varchar(128) NOT NULL,
    model varchar(255) NOT NULL,
    task_type varchar(16) NOT NULL CHECK (task_type IN ('text','image')),
    currency varchar(16) NOT NULL DEFAULT 'USD',
    price_per_request double precision CHECK (price_per_request IS NULL OR (price_per_request >= 0.0 AND price_per_request <= '1e15'::float8)),
    tiered_pricing jsonb,
    effective_from_unix_secs bigint NOT NULL CHECK (effective_from_unix_secs >= 0),
    effective_to_unix_secs bigint CHECK (effective_to_unix_secs > effective_from_unix_secs),
    created_by varchar(128) NOT NULL DEFAULT '',
    created_at_unix_secs bigint NOT NULL CHECK (created_at_unix_secs >= 0),
    updated_at_unix_secs bigint NOT NULL CHECK (updated_at_unix_secs >= 0),
    CHECK (price_per_request IS NOT NULL OR tiered_pricing IS NOT NULL)
);

CREATE INDEX IF NOT EXISTS provider_costs_lookup_idx
ON public.provider_costs (provider_id, model, task_type, effective_from_unix_secs DESC);
