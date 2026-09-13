WITH provider_facts AS (
  SELECT facts.provider_api_key_id, facts.status, facts.status_code, facts.error_message,
         facts.total_tokens, facts.total_cost_usd, facts.response_time_ms, facts.created_at
  FROM usage_billing_facts AS facts
  JOIN usage AS parent ON parent.request_id = facts.request_id
  WHERE parent.billing_mode = 'legacy'
  UNION ALL
  SELECT r.provider_api_key_id,
         COALESCE(r.terminal_facts #>> '{execution,status}', 'pending'),
         NULL::integer, NULL::text,
         CASE WHEN r.terminal_facts #>> '{outcome,kind}' = 'charged' THEN
           COALESCE((r.terminal_facts #>> '{outcome,usage,input_tokens}')::bigint, 0)
           + COALESCE((r.terminal_facts #>> '{outcome,usage,output_tokens}')::bigint, 0)
           + COALESCE((r.terminal_facts #>> '{outcome,usage,cache_creation_tokens}')::bigint, 0)
           + COALESCE((r.terminal_facts #>> '{outcome,usage,cache_read_tokens}')::bigint, 0)
         ELSE 0 END,
         CASE WHEN r.terminal_facts #>> '{outcome,kind}' = 'charged'
              THEN (r.terminal_facts #>> '{outcome,usage,total_cost_units}')::numeric / 100000000
              ELSE 0 END,
         (r.terminal_facts #>> '{execution,response_time_ms}')::bigint,
         to_timestamp((r.quote ->> 'admitted_at_unix_secs')::double precision)
  FROM request_fund_reservations r
  WHERE r.attempt_id IS NOT NULL AND r.dispatched_at IS NOT NULL
), aggregated AS (
  SELECT
    provider_api_key_id,
    COUNT(*)::BIGINT AS request_count,
    COALESCE(SUM(
      CASE
        WHEN status IN ('completed', 'success', 'ok', 'billed', 'settled')
             AND (status_code IS NULL OR status_code < 400)
             AND NULLIF(BTRIM(error_message), '') IS NULL
        THEN 1
        ELSE 0
      END
    ), 0)::BIGINT AS success_count,
    COALESCE(SUM(
      CASE
        WHEN status NOT IN ('pending', 'streaming')
             AND NOT (
               status IN ('completed', 'success', 'ok', 'billed', 'settled')
               AND (status_code IS NULL OR status_code < 400)
               AND NULLIF(BTRIM(error_message), '') IS NULL
             )
        THEN 1
        ELSE 0
      END
    ), 0)::BIGINT AS error_count,
    COALESCE(SUM(
      CASE
        WHEN status IN ('pending', 'streaming') THEN 0
        ELSE GREATEST(
          COALESCE(total_tokens, 0),
          0
        )::BIGINT
      END
    ), 0)::BIGINT AS total_tokens,
    COALESCE(SUM(
      CASE
        WHEN status IN ('pending', 'streaming') THEN 0
        ELSE COALESCE(total_cost_usd, 0)
      END
    ), 0)::NUMERIC(20,8) AS total_cost_usd,
    COALESCE(SUM(
      CASE
        WHEN status IN ('completed', 'success', 'ok', 'billed', 'settled')
             AND (status_code IS NULL OR status_code < 400)
             AND NULLIF(BTRIM(error_message), '') IS NULL
             AND response_time_ms IS NOT NULL
        THEN GREATEST(response_time_ms, 0)
        ELSE 0
      END
    ), 0)::BIGINT AS total_response_time_ms,
    MAX(created_at) AS last_used_at
  FROM provider_facts AS "usage"
  WHERE provider_api_key_id IS NOT NULL
    AND BTRIM(provider_api_key_id) <> ''
  GROUP BY provider_api_key_id
)
UPDATE provider_api_keys
SET
  request_count = aggregated.request_count,
  success_count = aggregated.success_count,
  error_count = aggregated.error_count,
  total_tokens = aggregated.total_tokens,
  total_cost_usd = aggregated.total_cost_usd,
  total_response_time_ms = aggregated.total_response_time_ms,
  last_used_at = aggregated.last_used_at
FROM aggregated
WHERE provider_api_keys.id = aggregated.provider_api_key_id
