-- Empty databases do not need the incremental migration: the snapshot runs
-- before traffic starts and executes inside a transaction.
ALTER TABLE public.api_keys
    ADD COLUMN IF NOT EXISTS daily_usage_limit_usd double precision;

ALTER TABLE public.user_groups
    ADD COLUMN IF NOT EXISTS daily_usage_limit_usd double precision,
    ADD COLUMN IF NOT EXISTS daily_usage_limit_mode text NOT NULL DEFAULT 'inherit';

DO $snapshot$
BEGIN
  ALTER TABLE public.user_groups
    ADD CONSTRAINT user_groups_daily_usage_limit_mode_check
    CHECK (daily_usage_limit_mode IN ('inherit', 'system', 'custom'));
EXCEPTION WHEN duplicate_object THEN NULL;
END $snapshot$;

-- Keep the built-in default group on the system-wide daily usage limit, matching
-- the rate_limit_mode = 'system' seed in 006_footer.sql.
UPDATE public.user_groups
    SET daily_usage_limit_mode = 'system'
    WHERE id = '00000000-0000-0000-0000-000000000001';
