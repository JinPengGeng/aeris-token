# Billing formula finite-result decision

Date: 2026-09-12

## Finding

`formula_engine.rs` previously used native `f64` arithmetic for `/`, `//`, `%`,
power, and the other arithmetic operators. Division or remainder by zero and
overflow could therefore produce `NaN` or infinity. The negative-cost check did
not catch `NaN`, and `quantize_cost` intentionally preserves a non-finite input,
so a complete billing result could reach downstream settlement and be rejected
later into the DLQ.

Quantization also multiplies by the storage-precision factor before rounding.
A finite but excessively large formula result could overflow in that step and
become infinite.

## Decision

- Reject a non-finite literal, variable, arithmetic intermediate, function
  result, or final expression through the existing
  `ExpressionEvaluationError::Failed` path.
- Verify total cost and `_cost` breakdown values remain finite after the
  existing quantization operation.
- Preserve current semantics for finite zero and negative final costs: zero is
  complete, while a negative result is incomplete with `negative_cost`.
- Do not change storage precision, rounding, or the optional-computed-variable
  fallback contract in this change.

Failing at evaluation keeps malformed formula outcomes out of the successful
billing result path and provides a deterministic configuration/evaluation error
at the source instead of a later serialization or settlement failure.

## Verification

Unit coverage in `formula_engine.rs` exercises `/`, `//`, and `%` with a zero
divisor, masked non-finite intermediates, arithmetic overflow, quantization
overflow, a large finite boundary value, and the existing zero/negative cost
contracts. The complete `aether-billing` library test target is the acceptance
gate.
