-- Fork-owned table (Refs #546): store provider cost price_per_request as
-- NUMERIC(20,8) instead of double precision so admin-entered decimals round
-- trip exactly. Existing float8 values convert via numeric rounding at 8
-- fractional digits. tiered_pricing stays JSONB: it is deliberately
-- isomorphic to the sales-side models.tiered_pricing catalog, whose shape
-- and validation are shared f64 JSON numbers (converting embedded floats in
-- SQL would fork the catalog schema from the sales side for no precision
-- win at the 1e-6/1M-token price magnitudes involved).
-- Rollback:
-- ALTER TABLE public.provider_costs DROP CONSTRAINT provider_costs_price_per_request_check;
-- ALTER TABLE public.provider_costs ALTER COLUMN price_per_request TYPE double precision
--   USING price_per_request::double precision;
-- ALTER TABLE public.provider_costs ADD CONSTRAINT provider_costs_price_per_request_check
--   CHECK (price_per_request IS NULL OR (price_per_request >= 0.0 AND price_per_request <= '1e15'::float8));
ALTER TABLE public.provider_costs DROP CONSTRAINT IF EXISTS provider_costs_price_per_request_check;
ALTER TABLE public.provider_costs
    ALTER COLUMN price_per_request TYPE numeric(20,8)
    USING round(price_per_request::numeric, 8);
ALTER TABLE public.provider_costs ADD CONSTRAINT provider_costs_price_per_request_check
    CHECK (price_per_request IS NULL OR (price_per_request >= 0 AND price_per_request <= 1000000000000000));
