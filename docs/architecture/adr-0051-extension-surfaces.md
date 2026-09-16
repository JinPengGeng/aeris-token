# ADR-0051: Provider and data-layer extension surfaces

## Status

Accepted for Issue #222 as a bounded architecture contract. This record captures
the current PostgreSQL-only data boundary and the provider registration points;
it does not claim that all provider behavior has been unified or that migration
history has been rewritten.

## Evidence and decision

The old MySQL/SQLite implementation matrix was removed by commit
`2281f2b754551c3a2cb3586486e9c733cb54ab60`. The current contract is one SQL
driver (`Postgres`) in `aether-data-contracts`, with the runtime facade selecting
the PostgreSQL adapter. Physical migrations remain under
`adapters/postgres/migrations`; logical definitions live under `schema/logical`,
generated audit output under `schema/generated`, and `compose_schema.sh check`
is the drift guard. A new table must update the logical definition first and
then the executable PostgreSQL migration when deployment compatibility requires
it.

Provider behavior is intentionally split by capability, but each capability has
an explicit registration point:

| Surface | Registration point | Contract |
| --- | --- | --- |
| Fixed transport metadata | `crates/aether-provider/transport/src/provider_types.rs` | `FixedProviderTemplate`, `fixed_provider_template`, and `provider_runtime_policy` are the source of truth for fixed provider defaults. |
| Pool/quota behavior | `crates/aether-provider/pool/src/service.rs` and `providers/mod.rs` | `ProviderPoolService::with_builtin_adapters` is the pool registry; unknown types use the default adapter. |
| OAuth behavior | `crates/aether-oauth/src/provider/service.rs` and `providers/*` | `ProviderOAuthService::with_builtin_adapters` registers dedicated adapters and generic templates. |
| Repository contract | `crates/aether-data/contracts/src/repository` | Traits remain SQLx-independent; selected adapters implement them. |

The fixed provider inventory is deliberately listed here so a new template
cannot silently bypass the architecture review:

| Provider type | Pool or OAuth follow-up |
| --- | --- |
| `claude_code` | unsupported pool adapter; dedicated OAuth adapter |
| `codex` | dedicated pool and OAuth adapters |
| `chatgpt_web` | dedicated pool adapter; generic OAuth template |
| `kiro` | dedicated pool and OAuth adapters |
| `grok` | dedicated pool adapter; generic OAuth behavior where applicable |
| `gemini_cli` | generic/unsupported pool behavior; generic OAuth template |
| `vertex_ai` | unsupported pool adapter; no OAuth adapter |
| `antigravity` | unsupported pool adapter; dedicated OAuth adapter |
| `windsurf` | dedicated pool and OAuth adapters |
| `xai` | dedicated pool and OAuth adapters |

## Measured change surface

The historical Grok integration is a reproducible baseline for future work. Its
three commits touched 124 files in total:

| Commit | Scope | Files | Insertions | Deletions |
| --- | --- | ---: | ---: | ---: |
| `cbfe1d378` | pool and transport | 20 | 2,616 | 21 |
| `936e1ae37` | admin OAuth and quota | 61 | 4,747 | 286 |
| `5bf236957` | runtime image surfaces | 43 | 7,044 | 400 |

The baseline is evidence for measuring a later registry slice, not a target for
a broad rewrite. The accepted settlement funding sample (`ab1dfa968`) touched
21 files because its contract, PostgreSQL adapter, memory implementation,
schema, and tests were changed together.

## Change checklist and verification

For a provider change, update only the capability surfaces it actually needs,
then run the corresponding crate tests and the gateway architecture tests. For
a repository or schema change, update the SQLx-independent contract, selected
adapter(s), memory tests where present, logical schema, generated output, and
the PostgreSQL migration; run `compose_schema.sh check` and the affected live
or unit tests.

The executable contract is
`apps/aether-gateway/src/tests/architecture/issue_222.rs`. It verifies that
fixed-provider metadata, pool and OAuth registries remain explicit, that every
fixed template is recorded in this ADR, and that the data layer keeps the
PostgreSQL-only boundary and documented migration sources.

## Deferred scope

No generic provider registry spanning quota, model fetch, planner, admin import,
and transport is introduced here. No trait splitting or migration squashing is
approved by this ADR. Those changes require a concrete provider or repository
sample with a before/after file and test measurement.
