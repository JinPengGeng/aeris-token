-- Keep provider_cost_snapshots as the one-row current state for each logical
-- request cost. Import receipts are immutable so replays retain their source.
CREATE TABLE IF NOT EXISTS public.provider_cost_snapshot_imports (
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
    occurred_at_unix_secs bigint NOT NULL CHECK (occurred_at_unix_secs >= 0),
    imported_by varchar(128) NOT NULL,
    imported_at timestamptz NOT NULL DEFAULT now(),
    CHECK (
        (certainty = 'unknown' AND provider_cost_amount_units IS NULL AND provider_currency IS NULL AND price_import_id IS NULL AND price_version IS NULL)
        OR
        (certainty = 'estimated' AND source_kind = 'estimate' AND provider_cost_amount_units IS NOT NULL AND provider_currency IS NOT NULL AND price_import_id IS NOT NULL AND price_version IS NOT NULL AND source_reference IS NOT NULL)
        OR
        (certainty = 'known' AND source_kind = 'supplier_bill' AND provider_cost_amount_units IS NOT NULL AND provider_currency IS NOT NULL AND source_reference IS NOT NULL)
    )
);

CREATE INDEX IF NOT EXISTS provider_cost_snapshot_imports_identity_idx
ON public.provider_cost_snapshot_imports (request_id, provider, model, dimension, imported_at);

-- Existing snapshots predate immutable receipts. Preserve each as its original
-- import receipt before subsequent imports may advance the current state.
INSERT INTO public.provider_cost_snapshot_imports (
    import_id, request_id, provider, model, dimension, sales_amount_units,
    sales_currency, provider_cost_amount_units, provider_currency, certainty,
    source_kind, reconciliation_status, price_import_id, price_version,
    source_reference, occurred_at_unix_secs, imported_by, imported_at
)
SELECT
    import_id, request_id, provider, model, dimension, sales_amount_units,
    sales_currency, provider_cost_amount_units, provider_currency, certainty,
    source_kind, reconciliation_status, price_import_id, price_version,
    source_reference, occurred_at_unix_secs, imported_by, imported_at
FROM public.provider_cost_snapshots
ON CONFLICT (import_id) DO NOTHING;
