# Billing formula function allowlist decision

Date: 2026-09-12

## Finding

The billing evaluator and the admin write-time validator each maintained the
same six function names. Adding a function could therefore make a valid engine
expression fail validation before it was stored.

Formula identifiers are deliberately not a global allowlist: billing rules
define variables through their own `variables` and `dimension_mappings`.
Those schema-owned names must remain extensible. The shared contract is limited
to built-in function names.

## Decision

- `aether-billing::FORMULA_ALLOWED_FUNCTIONS` is the sole declaration of
  built-in formula functions.
- The engine guards dispatch through `is_formula_function_allowed` before
  evaluating a call.
- The admin expression validator calls that same helper for every identifier
  followed by `(`.
- Do not add an independent admin allowlist or a static variable allowlist.

## Verification

The billing crate evaluates every exported function name. The gateway test
iterates the same exported list through admin validation and verifies an
unlisted function is rejected. Adding a built-in function now changes one
declaration and requires both checks to pass.

## Completion review (2026-09-13)

PR #304 merged the shared declaration. Its review correctly noted that a future
allowlist entry without an evaluator branch would reach `unreachable!`. The
fallback now returns `Unsupported`; the per-function test requires an expected
numeric result and rejects missing arguments for every exported name. The engine
also directly tests rejection of an unlisted function. Admin still uses the same
helper and its existing acceptance/rejection tests.

Adding a function means adding its name to the one allowlist, implementing its
semantics, and adding a result fixture. The admin validator needs no new list or
dispatch arm. This work preserves current function arity behavior; changes to the
formula language or dependency architecture belong to #226. The full #229
acceptance and documentation decisions are recorded in
[the completion review](issue-229-completion.md).
