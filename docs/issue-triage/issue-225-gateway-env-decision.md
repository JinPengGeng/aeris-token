# Issue 225 gateway environment contract

The gateway environment reference is generated from Rust source rather than
maintained as an independent hand-written allowlist. This is deliberate:
`Args` contains flattened clap groups and data subcommands, while many tuning
knobs are read by runtime modules with `std::env::var` and therefore do not
appear in clap help.

`tools/check_gateway_env_reference.py` scans both forms, excludes test-only
fixtures, and compares the source set with
`docs/operations/gateway-environment-reference.md`. A source addition must
regenerate the table in the same change; CI or a local pre-commit check can run
the script without `--write` to detect drift.

Defaults are recorded as the clap expression (including automatic sizing
where applicable); runtime-only values intentionally show `unset` because
their fallback and enable/disable semantics live in the owning module. Units
are inferred from stable suffixes (`_MS`, `_SECS`, `_BYTES`, `_MB`) and should
be read together with the source validation rules.
