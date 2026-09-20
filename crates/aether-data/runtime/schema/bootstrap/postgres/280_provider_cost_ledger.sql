-- Rollback (destructive): DROP TABLE provider_cost_snapshots, then provider_cost_prices.
-- No backfill is performed. These tables never update wallet or sales settlement facts.
CREATE TABLE IF NOT EXISTS public.provider_cost_prices (
    import_id varchar(128) PRIMARY KEY,
    supplier varchar(128) NOT NULL,
    provider varchar(128) NOT NULL,
    model varchar(255) NOT NULL,
    dimension varchar(32) NOT NULL CHECK (dimension IN ('input','output','cache_read','cache_write','image','request')),
    currency varchar(16) NOT NULL,
    unit varchar(32) NOT NULL CHECK (unit IN ('per_million_tokens','per_image','per_request')),
    version varchar(128) NOT NULL,
    price_units bigint NOT NULL CHECK (price_units >= 0),
    effective_from_unix_secs bigint NOT NULL CHECK (effective_from_unix_secs >= 0),
    effective_to_unix_secs bigint CHECK (effective_to_unix_secs > effective_from_unix_secs),
    source_reference varchar(255) NOT NULL,
    imported_by varchar(128) NOT NULL,
    imported_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (supplier, provider, model, dimension, currency, unit, version)
);

CREATE INDEX IF NOT EXISTS provider_cost_prices_effective_idx
ON public.provider_cost_prices (
    supplier, provider, model, dimension, currency, unit, effective_from_unix_secs
);

CREATE TABLE IF NOT EXISTS public.provider_cost_snapshots (
    import_id varchar(128) PRIMARY KEY,
    request_id varchar(128) NOT NULL,
    provider varchar(128) NOT NULL,
    model varchar(255) NOT NULL,
    dimension varchar(32) NOT NULL CHECK (dimension IN ('input','output','cache_read','cache_write','image','request')),
    sales_amount_units bigint NOT NULL CHECK (sales_amount_units >= 0),
    sales_currency varchar(16) NOT NULL,
    provider_cost_amount_units bigint CHECK (provider_cost_amount_units >= 0),
    provider_currency varchar(16),
    certainty varchar(16) NOT NULL CHECK (certainty IN ('known','estimated','unknown')),
    source_kind varchar(24) NOT NULL CHECK (source_kind IN ('supplier_bill','manual_import','estimate')),
    reconciliation_status varchar(24) NOT NULL CHECK (reconciliation_status IN ('unreconciled','matched','disputed','not_applicable')),
    price_import_id varchar(128) REFERENCES public.provider_cost_prices(import_id),
    price_version varchar(128),
    source_reference varchar(255),
    price_components jsonb NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(price_components) = 'array'),
    occurred_at_unix_secs bigint NOT NULL CHECK (occurred_at_unix_secs >= 0),
    imported_by varchar(128) NOT NULL,
    imported_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (request_id, provider, model, dimension),
    CHECK (
        (certainty = 'unknown' AND provider_cost_amount_units IS NULL AND provider_currency IS NULL AND price_import_id IS NULL AND price_version IS NULL AND price_components = '[]'::jsonb)
        OR
        (certainty = 'estimated' AND source_kind = 'estimate' AND provider_cost_amount_units IS NOT NULL AND provider_currency IS NOT NULL AND source_reference IS NOT NULL AND ((jsonb_array_length(price_components) = 0 AND price_import_id IS NOT NULL AND price_version IS NOT NULL) OR (dimension = 'request' AND jsonb_array_length(price_components) > 0)))
        OR
        (certainty = 'known' AND source_kind = 'supplier_bill' AND provider_cost_amount_units IS NOT NULL AND provider_currency IS NOT NULL AND source_reference IS NOT NULL)
    )
);

CREATE INDEX IF NOT EXISTS provider_cost_snapshots_summary_idx
ON public.provider_cost_snapshots (occurred_at_unix_secs, sales_currency, provider_currency, certainty, price_version, reconciliation_status);

CREATE INDEX IF NOT EXISTS provider_cost_snapshots_request_idx
ON public.provider_cost_snapshots (request_id, occurred_at_unix_secs);
