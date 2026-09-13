-- Deploy with old usage/funds writers drained. The backfill and first switch to
-- ledger reads are one upgrade boundary; see docs/operations/daily-cost-ledger.md.
CREATE TABLE public.usage_daily_cost_contributions (
    request_id varchar(128) PRIMARY KEY,
    usage_id varchar(36) NOT NULL,
    user_id varchar(128),
    api_key_id varchar(128),
    api_key_is_standalone boolean NOT NULL,
    attempt_funds boolean NOT NULL,
    actual_cost_units bigint NOT NULL CHECK (actual_cost_units >= 0),
    accounting_at timestamptz,
    updated_at timestamptz NOT NULL DEFAULT now()
);
-- Intentionally no cascading foreign keys: audit deletion does not refund cost
-- or erase the request identity needed to reject archived event replays.
CREATE INDEX usage_daily_cost_key_window_idx
    ON public.usage_daily_cost_contributions (api_key_id, accounting_at)
    INCLUDE (actual_cost_units) WHERE accounting_at IS NOT NULL;
CREATE INDEX usage_daily_cost_user_window_idx
    ON public.usage_daily_cost_contributions (user_id, accounting_at)
    INCLUDE (actual_cost_units)
    WHERE accounting_at IS NOT NULL AND NOT api_key_is_standalone;

-- Also an executable, repeatable repair after draining all old writers. It
-- derives attempt costs from durable integer child facts, not display floats.
CREATE FUNCTION public.backfill_usage_daily_cost_contributions() RETURNS bigint
LANGUAGE plpgsql AS $$
DECLARE touched bigint;
BEGIN
    LOCK TABLE public.usage IN ACCESS EXCLUSIVE MODE;
    LOCK TABLE public.usage_daily_cost_contributions IN ACCESS EXCLUSIVE MODE;
    IF EXISTS (
        SELECT 1 FROM public.usage u JOIN public.usage_daily_cost_contributions d
          ON d.request_id = u.request_id WHERE d.usage_id <> u.id
    ) THEN
        RAISE EXCEPTION 'daily cost request identity was reused after audit cleanup';
    END IF;
    IF EXISTS (
        SELECT 1 FROM public.usage u JOIN public.usage_daily_cost_contributions d
          ON d.request_id = u.request_id
        WHERE u.billing_mode = 'attempt_funds' AND NOT d.attempt_funds
          AND (d.accounting_at IS NOT NULL OR d.actual_cost_units <> 0)
    ) THEN
        RAISE EXCEPTION 'cannot convert accounted legacy daily cost to attempt funds';
    END IF;
    INSERT INTO public.usage_daily_cost_contributions AS d
      (request_id, usage_id, user_id, api_key_id, api_key_is_standalone,
       attempt_funds, actual_cost_units, accounting_at)
    SELECT u.request_id, u.id, u.user_id, u.api_key_id,
      COALESCE(u.request_metadata->>'api_key_is_standalone', 'false') = 'true',
      u.billing_mode = 'attempt_funds',
      CASE WHEN u.billing_mode = 'attempt_funds' THEN COALESCE((
        SELECT SUM((f.terminal_facts->'outcome'->'usage'->>'actual_cost_units')::bigint)::bigint
        FROM public.request_fund_reservations f
        WHERE f.request_id = u.request_id AND f.attempt_id IS NOT NULL
          AND f.terminal_facts->'outcome'->>'kind' = 'charged'
      ), 0)
      WHEN u.status = 'completed' THEN ROUND(COALESCE(s.billing_actual_total_cost_usd,
        u.actual_total_cost_usd, 0)::numeric * 100000000)::bigint
      ELSE 0 END,
      CASE WHEN u.billing_mode = 'attempt_funds' THEN u.funds_admission_closed_at
        WHEN u.status = 'completed' THEN COALESCE(s.finalized_at, u.finalized_at,
          to_timestamp(NULLIF(u.updated_at_unix_secs, 0)::double precision), u.created_at)
        ELSE NULL END
    FROM public.usage u LEFT JOIN public.usage_settlement_snapshots s ON s.request_id = u.request_id
    ON CONFLICT (request_id) DO UPDATE SET
      attempt_funds = d.attempt_funds OR EXCLUDED.attempt_funds,
      actual_cost_units = CASE
        WHEN EXCLUDED.attempt_funds OR EXCLUDED.accounting_at IS NOT NULL
        THEN EXCLUDED.actual_cost_units ELSE d.actual_cost_units END,
      accounting_at = COALESCE(d.accounting_at, EXCLUDED.accounting_at),
      updated_at = now();
    GET DIAGNOSTICS touched = ROW_COUNT;
    RETURN touched;
END;
$$;
SELECT public.backfill_usage_daily_cost_contributions();
