-- PostgreSQL NUMERIC special-value boundary fixture.
-- Read-only: pg_input_is_valid validates typmod acceptance without inserting
-- values into application tables or raising an expected conversion error.

-- NUMERIC without precision/scale accepts the PostgreSQL special values.
WITH special_values(value) AS (
  VALUES ('Infinity'::numeric), ('-Infinity'::numeric), ('NaN'::numeric)
)
SELECT
  value::text AS value,
  value = 'Infinity'::numeric AS is_positive_infinity,
  value = '-Infinity'::numeric AS is_negative_infinity,
  value = 'NaN'::numeric AS is_nan
FROM special_values
ORDER BY value::text;

-- Application money columns use numeric(20,8). These values must be rejected
-- by the declared precision/scale contract. This query is non-throwing.
WITH special_values(value) AS (
  VALUES ('Infinity'), ('-Infinity'), ('NaN')
)
SELECT
  value,
  pg_input_is_valid(value, 'numeric(20,8)') AS accepted_by_money_typmod
FROM special_values
ORDER BY value;

-- Expected: all three accepted_by_money_typmod values are false. If any is
-- true, review the database major version and the schema typmod before using
-- the historical NUMERIC audit conclusions.
