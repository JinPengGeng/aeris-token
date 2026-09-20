-- Keep legacy single-price estimates valid while allowing a request-level
-- aggregate to retain the individual effective-price references it used.
ALTER TABLE public.provider_cost_snapshots
    ADD COLUMN IF NOT EXISTS price_components jsonb NOT NULL DEFAULT '[]'::jsonb;

ALTER TABLE public.provider_cost_snapshot_imports
    ADD COLUMN IF NOT EXISTS price_components jsonb NOT NULL DEFAULT '[]'::jsonb;

ALTER TABLE public.provider_cost_snapshots
    DROP CONSTRAINT IF EXISTS provider_cost_snapshots_check;
ALTER TABLE public.provider_cost_snapshots
    DROP CONSTRAINT IF EXISTS provider_cost_snapshots_price_components_check;
ALTER TABLE public.provider_cost_snapshot_imports
    DROP CONSTRAINT IF EXISTS provider_cost_snapshot_imports_check;
ALTER TABLE public.provider_cost_snapshot_imports
    DROP CONSTRAINT IF EXISTS provider_cost_snapshot_imports_price_components_check;

ALTER TABLE public.provider_cost_snapshots
    ADD CONSTRAINT provider_cost_snapshots_price_components_check CHECK (
        jsonb_typeof(price_components) = 'array'
        AND (
            (certainty = 'unknown'
                AND provider_cost_amount_units IS NULL
                AND provider_currency IS NULL
                AND price_import_id IS NULL
                AND price_version IS NULL
                AND price_components = '[]'::jsonb)
            OR
            (certainty = 'estimated'
                AND source_kind = 'estimate'
                AND provider_cost_amount_units IS NOT NULL
                AND provider_currency IS NOT NULL
                AND source_reference IS NOT NULL
                AND (
                    (jsonb_array_length(price_components) = 0
                        AND price_import_id IS NOT NULL
                        AND price_version IS NOT NULL)
                    OR (dimension = 'request' AND jsonb_array_length(price_components) > 0)
                ))
            OR
            (certainty = 'known'
                AND source_kind = 'supplier_bill'
                AND provider_cost_amount_units IS NOT NULL
                AND provider_currency IS NOT NULL
                AND source_reference IS NOT NULL)
        )
    );

ALTER TABLE public.provider_cost_snapshot_imports
    ADD CONSTRAINT provider_cost_snapshot_imports_price_components_check CHECK (
        jsonb_typeof(price_components) = 'array'
        AND (
            (certainty = 'unknown'
                AND provider_cost_amount_units IS NULL
                AND provider_currency IS NULL
                AND price_import_id IS NULL
                AND price_version IS NULL
                AND price_components = '[]'::jsonb)
            OR
            (certainty = 'estimated'
                AND source_kind = 'estimate'
                AND provider_cost_amount_units IS NOT NULL
                AND provider_currency IS NOT NULL
                AND source_reference IS NOT NULL
                AND (
                    (jsonb_array_length(price_components) = 0
                        AND price_import_id IS NOT NULL
                        AND price_version IS NOT NULL)
                    OR (dimension = 'request' AND jsonb_array_length(price_components) > 0)
                ))
            OR
            (certainty = 'known'
                AND source_kind = 'supplier_bill'
                AND provider_cost_amount_units IS NOT NULL
                AND provider_currency IS NOT NULL
                AND source_reference IS NOT NULL)
        )
    );
