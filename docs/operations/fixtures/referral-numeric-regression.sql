-- Adapter regression fixture for NUMERIC -> f64 reads.
-- This fixture is intentionally self-contained and read-only after the
-- temporary table is created. It can be pasted into a PostgreSQL integration
-- test or run in an isolated session.

CREATE TEMP TABLE referral_numeric_fixture (
  amount_usd numeric(20,8) NOT NULL,
  reversed_amount_usd numeric(20,8) NOT NULL,
  pending_reversal_amount_usd numeric(20,8) NOT NULL
) ON COMMIT DROP;

INSERT INTO referral_numeric_fixture VALUES
  ('0.00000001', '0.00000000', '0.00000000'),
  ('1234567890.12345678', '0.00000001', '2.50000000');

-- The production adapter uses this explicit cast in every referral reward
-- SELECT. A direct f64 decode of the source NUMERIC columns must not be used.
SELECT
  CAST(amount_usd AS DOUBLE PRECISION) AS amount_usd,
  CAST(reversed_amount_usd AS DOUBLE PRECISION) AS reversed_amount_usd,
  CAST(pending_reversal_amount_usd AS DOUBLE PRECISION) AS pending_reversal_amount_usd
FROM referral_numeric_fixture
ORDER BY amount_usd;

-- Expected checks for an integration harness (all rows must be true).
SELECT
  COUNT(*) = 2 AS row_count_ok,
  MIN(CAST(amount_usd AS DOUBLE PRECISION)) = 0.00000001 AS min_amount_ok,
  MAX(CAST(amount_usd AS DOUBLE PRECISION)) = 1234567890.12345678 AS max_amount_ok,
  BOOL_AND(CAST(amount_usd AS DOUBLE PRECISION) IS NOT NULL) AS finite_decode_input
FROM referral_numeric_fixture;
