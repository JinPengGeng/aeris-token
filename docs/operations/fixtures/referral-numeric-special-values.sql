-- PostgreSQL NUMERIC special-value boundary fixture. Run with ON_ERROR_STOP=1.
-- Read-only: no application rows, schema, or persistent settings are changed.
BEGIN READ ONLY;
SET LOCAL statement_timeout = '5s';

DO $fixture$
DECLARE
  candidate record;
BEGIN
  FOR candidate IN
    SELECT * FROM (VALUES
      ('Infinity', false), ('-Infinity', false), ('NaN', true)
    ) AS expected(value, accepted_by_money_typmod)
  LOOP
    IF NOT pg_input_is_valid(candidate.value, 'numeric') THEN
      RAISE EXCEPTION 'unconstrained numeric rejected %', candidate.value;
    END IF;
    IF pg_input_is_valid(candidate.value, 'numeric(20,8)')
        IS DISTINCT FROM candidate.accepted_by_money_typmod THEN
      RAISE EXCEPTION 'unexpected numeric(20,8) acceptance for %', candidate.value;
    END IF;
  END LOOP;
  -- The cast proves NaN can inhabit the actual money typmod; precision alone
  -- cannot replace the historical audit's explicit nonfinite-value check.
  IF ('NaN'::numeric(20,8) = 'NaN'::numeric) IS NOT TRUE THEN
    RAISE EXCEPTION 'NaN must remain detectable by the historical audit';
  END IF;
END
$fixture$;

SELECT value, pg_input_is_valid(value, 'numeric(20,8)') AS accepted_by_money_typmod
FROM (VALUES ('Infinity'), ('-Infinity'), ('NaN')) AS special_values(value)
ORDER BY value;

ROLLBACK;
