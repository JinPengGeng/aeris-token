-- Read-only historical referral-reward audit for PostgreSQL.
-- Run with a least-privileged, read-only role. This script returns aggregates
-- only; it deliberately does not select user ids, order ids, or free text.

-- 1) Population, date range, and amount totals by durable status/type.
SELECT
  reward_type,
  status,
  COUNT(*) AS reward_count,
  MIN(created_at) AS first_created_at,
  MAX(created_at) AS last_created_at,
  SUM(CAST(amount_usd AS numeric(30,8))) AS amount_usd,
  SUM(CAST(reversed_amount_usd AS numeric(30,8))) AS reversed_amount_usd,
  SUM(CAST(pending_reversal_amount_usd AS numeric(30,8))) AS pending_reversal_amount_usd
FROM public.referral_rewards
GROUP BY reward_type, status
ORDER BY reward_type, status;

-- 2) Rows whose reward claims to be applied but has no linked wallet
-- transaction. This is an investigation queue, not a repair decision.
SELECT
  status,
  COUNT(*) AS rows_without_wallet_transaction,
  MIN(created_at) AS first_created_at,
  MAX(created_at) AS last_created_at,
  SUM(CAST(amount_usd AS numeric(30,8))) AS amount_usd
FROM public.referral_rewards
WHERE status = 'applied'
  AND wallet_transaction_id IS NULL
GROUP BY status;

-- 3) Compare durable reward amounts with the linked wallet ledger. The
-- comparison is aggregate-only and uses NUMERIC arithmetic end-to-end.
SELECT
  COUNT(*) AS linked_rows,
  COUNT(*) FILTER (
    WHERE ABS(CAST(rr.amount_usd AS numeric(30,8))
          - ABS(CAST(wt.amount AS numeric(30,8)))
         ) > CAST('0.00000001' AS numeric)
  ) AS amount_mismatch_rows,
  COALESCE(SUM(CAST(rr.amount_usd AS numeric(30,8))), 0) AS reward_total_usd,
  COALESCE(SUM(ABS(CAST(wt.amount AS numeric(30,8)))), 0) AS ledger_total_usd
FROM public.referral_rewards rr
JOIN public.wallet_transactions wt
  ON wt.id = rr.wallet_transaction_id
WHERE rr.wallet_transaction_id IS NOT NULL;

-- 4) Detect malformed numeric state without exposing row identity.
SELECT
  COUNT(*) FILTER (WHERE amount_usd < 0) AS negative_amount_rows,
  COUNT(*) FILTER (WHERE reversed_amount_usd < 0) AS negative_reversed_rows,
  COUNT(*) FILTER (WHERE pending_reversal_amount_usd < 0) AS negative_pending_reversal_rows,
  COUNT(*) FILTER (WHERE reversed_amount_usd > amount_usd) AS over_reversed_rows,
  COUNT(*) FILTER (WHERE pending_reversal_amount_usd > amount_usd - reversed_amount_usd)
    AS over_pending_reversal_rows
FROM public.referral_rewards;
