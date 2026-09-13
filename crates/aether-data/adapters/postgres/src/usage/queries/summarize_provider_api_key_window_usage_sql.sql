WITH provider_facts AS (
  SELECT facts.id, facts.provider_api_key_id, facts.total_tokens,
         facts.total_cost_usd, facts.created_at
  FROM usage_billing_facts AS facts
  JOIN usage AS parent ON parent.request_id = facts.request_id
  WHERE parent.billing_mode = 'legacy'
  UNION ALL
  SELECT r.reservation_token, r.provider_api_key_id,
         CASE WHEN r.terminal_facts #>> '{outcome,kind}' = 'charged' THEN
           COALESCE((r.terminal_facts #>> '{outcome,usage,input_tokens}')::bigint, 0)
           + COALESCE((r.terminal_facts #>> '{outcome,usage,output_tokens}')::bigint, 0)
           + COALESCE((r.terminal_facts #>> '{outcome,usage,cache_creation_tokens}')::bigint, 0)
           + COALESCE((r.terminal_facts #>> '{outcome,usage,cache_read_tokens}')::bigint, 0)
         ELSE 0 END,
         CASE WHEN r.terminal_facts #>> '{outcome,kind}' = 'charged'
              THEN (r.terminal_facts #>> '{outcome,usage,total_cost_units}')::numeric / 100000000
              ELSE 0 END,
         to_timestamp((r.quote ->> 'admitted_at_unix_secs')::double precision)
  FROM request_fund_reservations r
  WHERE r.attempt_id IS NOT NULL AND r.dispatched_at IS NOT NULL
), requested AS (
  SELECT
    request_row.provider_api_key_id,
    request_row.window_code,
    request_row.start_unix_secs,
    request_row.end_unix_secs,
    request_row.ordinality
  FROM UNNEST(
    $1::TEXT[],
    $2::TEXT[],
    $3::BIGINT[],
    $4::BIGINT[]
  ) WITH ORDINALITY AS request_row(
    provider_api_key_id,
    window_code,
    start_unix_secs,
    end_unix_secs,
    ordinality
  )
)
SELECT
  requested.provider_api_key_id,
  requested.window_code,
  COUNT("usage".id)::BIGINT AS request_count,
  COALESCE(SUM("usage".total_tokens), 0)::BIGINT AS total_tokens,
  CAST(COALESCE(SUM("usage".total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost_usd
FROM requested
LEFT JOIN provider_facts AS "usage"
  ON "usage".provider_api_key_id = requested.provider_api_key_id
 AND "usage".created_at >= to_timestamp(requested.start_unix_secs::DOUBLE PRECISION)
 AND "usage".created_at < to_timestamp(requested.end_unix_secs::DOUBLE PRECISION)
GROUP BY
  requested.provider_api_key_id,
  requested.window_code,
  requested.ordinality
ORDER BY requested.ordinality ASC
