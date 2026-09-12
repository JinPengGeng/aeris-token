-- Read-only historical referral-reward audit for PostgreSQL.
-- Run with a least-privileged, read-only role. This script returns aggregates
-- only; it deliberately does not select user ids, order ids, or free text.
-- Execute the whole file in a fresh session with ON_ERROR_STOP enabled.
BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;
SET LOCAL statement_timeout = '30s';
SET LOCAL lock_timeout = '2s';

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

-- 2) Missing applied-credit links and dangling ledger identifiers. The schema
-- does not enforce a foreign key on wallet_transaction_id.
SELECT
  rr.status,
  COUNT(*) AS rows_without_wallet_transaction,
  MIN(rr.created_at) AS first_created_at,
  MAX(rr.created_at) AS last_created_at,
  SUM(rr.amount_usd) AS amount_usd
FROM public.referral_rewards rr
LEFT JOIN public.wallet_transactions wt ON wt.id = rr.wallet_transaction_id
WHERE (rr.status IN ('applied', 'reversed') AND rr.wallet_transaction_id IS NULL)
   OR (rr.wallet_transaction_id IS NOT NULL AND wt.id IS NULL)
GROUP BY rr.status;

-- 3) Compare durable reward amounts with the linked wallet ledger. The
-- comparison is exact and signed: even one schema quantum is a difference,
-- and a debit of the same magnitude must never pass as a valid reward credit.
SELECT
  COUNT(*) AS linked_rows,
  COUNT(*) FILTER (WHERE rr.amount_usd IS DISTINCT FROM wt.amount)
    AS amount_mismatch_rows,
  COUNT(*) FILTER (
    WHERE wt.category IS DISTINCT FROM 'adjust'
       OR wt.reason_code IS DISTINCT FROM 'referral_reward'
       OR wt.link_type IS DISTINCT FROM 'referral_reward'
       OR wt.link_id IS DISTINCT FROM rr.id
  ) AS ledger_identity_mismatch_rows,
  COALESCE(SUM(rr.amount_usd), 0) AS reward_total_usd,
  COALESCE(SUM(wt.amount), 0) AS ledger_total_usd,
  COALESCE(SUM(rr.amount_usd - wt.amount), 0) AS amount_difference_usd
FROM public.referral_rewards rr
JOIN public.wallet_transactions wt
  ON wt.id = rr.wallet_transaction_id
WHERE rr.wallet_transaction_id IS NOT NULL;

-- 4) Detect malformed numeric state without exposing row identity.
SELECT
  COUNT(*) FILTER (
    WHERE amount_usd = 'NaN'::numeric
       OR reversed_amount_usd = 'NaN'::numeric
       OR pending_reversal_amount_usd = 'NaN'::numeric
  ) AS nonfinite_amount_rows,
  COUNT(*) FILTER (WHERE amount_usd < 0) AS negative_amount_rows,
  COUNT(*) FILTER (WHERE reversed_amount_usd < 0) AS negative_reversed_rows,
  COUNT(*) FILTER (WHERE pending_reversal_amount_usd < 0) AS negative_pending_reversal_rows,
  COUNT(*) FILTER (WHERE reversed_amount_usd > amount_usd) AS over_reversed_rows,
  COUNT(*) FILTER (WHERE pending_reversal_amount_usd > amount_usd - reversed_amount_usd)
    AS over_pending_reversal_rows
FROM public.referral_rewards;

COMMIT;
