-- Adapter regression fixture for NUMERIC -> f64 reads.
-- This fixture is intentionally self-contained and read-only after the
-- temporary table is created. It can be pasted into a PostgreSQL integration
-- test or run in an isolated session.
--
-- Boundary values: the production columns are numeric(20,8), so the absolute
-- value must round to less than 10^12 (12 integer digits). Values beyond
-- 2^53 (f64 integer-exactness boundary) can therefore never occur in this
-- schema; the realistic precision risk is 20-significant-digit values near
-- the numeric ceiling, which f64 cannot represent exactly.
BEGIN;

CREATE TEMP TABLE referral_numeric_fixture (
  amount_usd numeric(20,8) NOT NULL,
  reversed_amount_usd numeric(20,8) NOT NULL,
  pending_reversal_amount_usd numeric(20,8) NOT NULL
) ON COMMIT DROP;

INSERT INTO referral_numeric_fixture VALUES
  ('0.00000001', '0.00000000', '0.00000000'),
  ('1234567890.12345678', '0.00000001', '2.50000000'),
  ('999999999999.00000001', '0.00000000', '0.00000000'),
  ('123456789.12345678', '0.00000000', '0.00000000');

-- The production adapter uses this explicit cast in every referral reward
-- SELECT. A direct f64 decode of the source NUMERIC columns must not be used.
SELECT
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  CAST(reversed_amount_usd AS DOUBLE PRECISION) AS reversed_amount_usd,
  CAST(pending_reversal_amount_usd AS DOUBLE PRECISION) AS pending_reversal_amount_usd
FROM referral_numeric_fixture
ORDER BY amount_usd;

-- Expected checks for an integration harness (all rows must be true).
-- numeric_bounds_exact: NUMERIC comparisons retain every schema digit, which
-- is why the historical audit compares in NUMERIC, never in f64.
-- decode_precision_loss_visible: the f64 decode is an API compatibility cast;
-- it demonstrably drops digits on 20-significant-digit values, so it must
-- never be used for financial reconciliation or write-back.
SELECT
  COUNT(*) = 4 AS row_count_ok,
  MIN(CAST(amount_usd AS DOUBLE PRECISION)) = 0.00000001 AS min_amount_ok,
  MAX(CAST(amount_usd AS DOUBLE PRECISION)) = 999999999999.00000001 AS max_amount_ok,
  BOOL_AND(CAST(amount_usd AS DOUBLE PRECISION) IS NOT NULL) AS finite_decode_input,
  MIN(amount_usd) = '0.00000001'::numeric(20,8)
    AND MAX(amount_usd) = '999999999999.00000001'::numeric(20,8)
    AS numeric_bounds_exact,
  BOOL_OR(
    CAST(amount_usd AS DOUBLE PRECISION)::numeric(20,8) IS DISTINCT FROM amount_usd
  ) AS decode_precision_loss_visible
FROM referral_numeric_fixture;

ROLLBACK;
