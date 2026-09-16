# Issue #216: format field coverage gate

This slice adds the existing deterministic format-field matrix generator to the
required `Rust CI` shell fixtures. Changes to
`docs/api/provider-interface-definitions.md` or the generated coverage matrix
now fail the gate when the checked-in matrix is stale.

## Contract

The fixture runs:

```sh
PYTHONUTF8=1 python3 docs/api/generate_format_field_coverage.py --check
```

`PYTHONUTF8=1` keeps the generated Markdown comparison stable across hosted
runner locales. The generator uses only the Python standard library and its
default input/output paths are inside the repository.

This is a documentation and CI drift guard. It does not expand provider
support, change runtime conversion behavior, or prove the matrix is a complete
semantic compatibility review. New or changed fields still require the human
mapping decision recorded in the matrix.

## Verification

The command passes at the current head. The existing automation contract and
Rust CI aggregate gates remain unchanged; the shell fixture is already required
for both selected and full Rust runs.
