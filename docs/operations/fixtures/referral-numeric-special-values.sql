-- PostgreSQL NUMERIC special-value boundary fixture. Run with ON_ERROR_STOP=1.
-- Read-only: no application rows, schema, or persistent settings are changed.
BEGIN READ ONLY;
SET LOCAL statement_timeout = '5s';

DO $fixture$
DECLARE
  candidate record;
  constrained_value numeric(20,8);
  accepted boolean;
BEGIN
  FOR candidate IN
    SELECT * FROM (VALUES
      ('Infinity', false), ('-Infinity', false), ('NaN', true)
    ) AS expected(value, accepted_by_money_typmod)
  LOOP
    -- Real casts work on the deployed PostgreSQL 15 baseline as well as newer
    -- servers. pg_input_is_valid() was only added in PostgreSQL 16.
    IF candidate.value::numeric::text IS DISTINCT FROM candidate.value THEN
      RAISE EXCEPTION 'unconstrained numeric rejected %', candidate.value;
    END IF;
    accepted := true;
    BEGIN
      constrained_value := candidate.value::numeric(20,8);
    EXCEPTION WHEN numeric_value_out_of_range THEN
      accepted := false;
    END;
    IF accepted IS DISTINCT FROM candidate.accepted_by_money_typmod THEN
      RAISE EXCEPTION 'unexpected numeric(20,8) acceptance for %', candidate.value;
    END IF;
    RAISE NOTICE '% accepted by numeric(20,8): %', candidate.value, accepted;
  END LOOP;
  -- The cast proves NaN can inhabit the actual money typmod; precision alone
  -- cannot replace the historical audit's explicit nonfinite-value check.
  IF ('NaN'::numeric(20,8) = 'NaN'::numeric) IS NOT TRUE THEN
    RAISE EXCEPTION 'NaN must remain detectable by the historical audit';
  END IF;
END
$fixture$;

ROLLBACK;
