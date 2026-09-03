-- Empty databases do not need the incremental migration: the snapshot runs
-- before traffic starts and executes inside a transaction.
ALTER TABLE public.api_keys
    ADD COLUMN IF NOT EXISTS billing_multiplier numeric(10,6) NOT NULL DEFAULT 1.0;

DO $snapshot$
BEGIN
  ALTER TABLE public.api_keys
    ADD CONSTRAINT ck_api_keys_billing_multiplier_range
    CHECK (billing_multiplier >= 0 AND billing_multiplier <= 1000);
EXCEPTION WHEN duplicate_object THEN NULL;
END $snapshot$;
