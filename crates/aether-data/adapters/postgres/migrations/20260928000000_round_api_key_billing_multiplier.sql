-- Refs #553: migrate the key-level billing multiplier to fixed-point 4-decimal
-- semantics. The column itself stays numeric(10,6) because api_keys is an
-- upstream-owned table (no schema changes to upstream columns in this fork);
-- Rust code now reads/writes it rounded to 4 fractional digits, so existing
-- values with more precision are rounded once here. Values stay within the
-- existing 0..=1000 check constraint.
-- Rollback: not applicable for a lossy rounding; restore from backup.
UPDATE public.api_keys
SET billing_multiplier = round(billing_multiplier, 4)
WHERE billing_multiplier <> round(billing_multiplier, 4);
