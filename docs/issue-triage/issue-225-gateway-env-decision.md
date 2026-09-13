# Issue 225 gateway environment contract

The gateway environment reference is generated from Rust source rather than
maintained as an independent hand-written allowlist. This is deliberate:
`Args` contains flattened clap groups and data subcommands, while many tuning
knobs are read by runtime modules with `std::env::var` and therefore do not
appear in clap help.

`tools/check_gateway_env_reference.py` scans literal field-based clap
declarations, recognized environment-reader calls and ENV constants, excluding
fixture directories named `tests`. It compares the complete generated
`docs/operations/gateway-environment-reference.md`, including declared defaults,
field types, scope and validation attributes. New unsupported clap env syntax
fails explicitly. The required Rust CI shell job executes the regression test,
which also checks the current reference.

Defaults remain the declared source expression; the checker does not evaluate
Rust constants or read current environment values. Runtime readers are a
separate linked inventory without inferred types or defaults. In particular,
request candidate persistence uses `full` / `terminal` / `none` modes. Only the
13 explicitly global database arguments are described as inherited by data
subcommands; other root arguments and the standalone backup CLI are separate.

## Review correction and validation (2026-09-13)

Independent review rejected the earlier all-global labels, guessed boolean
types, missing update-timeout helpers and name-only drift check. The revised
projection corrects these issues and includes the exact declaration beside
each clap row. Tests change defaults, Rust types and scope without changing
the variable name, and add a helper-reader variable; each stale document fails.
The original source projection passes after regeneration. Four focused tests
passed, including the real 13-global-argument check and both update timeouts.

This is a bounded source reference, not an evaluation of all runtime defaults
or an inventory of dependency/crate-level environment settings. The parent
#225 remains open for those broader configuration and operational requirements.
