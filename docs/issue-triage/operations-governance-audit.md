# Operations and Governance Issue Revalidation

Snapshot: `12a1d265c` (2026-09-12). This is a current-tree review, not a
restatement of the original review issues. Status means: **fixed** is covered
by code and a focused test; **still valid** has a presently reachable gap;
**partial** has a narrower remaining problem; **decision** needs an explicit
product or architecture choice before code is written.

## Recommended Queue

| Order | Work item | Source issues | Benefit | Complexity | Exit criteria |
| --- | --- | --- | --- | --- | --- |
| P0 | Durable admin-mutation audit record, with bounded failure semantics | #255 | incident accountability | M | privileged mutation produces a queryable row and an integration test proves it |
| P1 | DLQ operator lifecycle (list, bounded retention, guarded redrive) | #223 | recover billable events safely | M | poison entry is visible, replayed exactly once, and retention is enforced |
| P1 | Balance-denial contract decision and compatibility tests | #247, #254 | stops retry ambiguity for clients | S | documented chosen HTTP/code/header contract across OpenAI and Claude routes |
| P1 | Operations documentation: multi-node, metrics, backup recovery | #224, #225, #223 | safe deployment and recovery | S | runnable instructions exercised in a CI/doc smoke test |
| P1 | Supply-chain gate: dependency advisory scan | #220 | known-CVE feedback before merge | S | pinned scanner runs on Cargo and each npm lockfile with an explicit advisory policy |
| P2 | Formula whitelist single source + numeric boundary tests | #226, #229 | prevents admin/editor divergence | S | one exported function list and finite-value tests |
| P2 | Contributor and security entry points | #256, #235 | lowers adoption and disclosure friction | S | CONTRIBUTING and SECURITY point to this fork and validated commands |
| P2 | Architecture roadmaps/ADRs | #221, #222, #235, #241 | makes future changes bounded | M | each ADR has a decision, owner, and staged acceptance slice |

## Per-Issue Revalidation

### #220 Supply chain

**Partial.** The original install-script integrity finding is fixed. The
installer downloads `SHA256SUMS` and calls `verify_release_checksum` before
extracting the archive (`install.sh:1475-1476`); the verifier requires exactly
one manifest entry and compares a local SHA-256 (`install.sh:744-761`).
Release workflows publish the manifest (`.github/workflows/release.yml:336`).
`Dockerfile.app` also pins both runtime images by digest
(`Dockerfile.app:13,30`). These facts invalidate the corresponding original
subclaims.

The current container still explicitly runs as UID 0 (`Dockerfile.app:47`).
The pinned `wreq` release candidate remains (`Cargo.toml:139`). A CI advisory
gate was not found in workflow definitions during this review. Treat root
runtime as a **decision** until volume ownership, file writes and a `nonroot`
image trial are verified; do not relabel it as an established vulnerability.

**Todo:** add a separately versioned advisory scan and policy file; then test
it with a known ignored advisory and a failing synthetic policy fixture. Keep
checksum and image-digest work closed. Benefit is high, implementation is
small.

### #221 Dependency layering

**Still valid as a long-term roadmap; not implementation-ready.** The app
crate remains the integration point and the proposed moves affect public test
and runtime wiring. Current architecture tests deliberately inspect source
shape (`apps/aether-gateway/src/tests/architecture/ai_serving.rs:3866`), so a
bulk move has non-local acceptance requirements.

**Todo:** first produce a dependency/call graph and select one independently
testable boundary (admin HTTP adapter is the candidate); record compile-time
dependency rules before moving code. Acceptance is a single extracted vertical
slice with unchanged endpoint integration tests. Benefit is medium-long-term;
complexity is high. Keep #221 in planning, rather than treating its estimates
as a bug backlog.

### #222 Contracts and extension cost

**Partial.** Repository and driver multiplication is a real maintenance cost,
but the proposed one-shot dialect/registry rewrite is not a justified next
task. The current data contract types retain driver independence, while the
existing provider and format code supplies many explicit compatibility checks.

**Decision/todo:** choose one actual provider addition or one repository method
as a measurement slice. Its changed-file count, driver tests, and migration
needs become the baseline for an ADR. Do not promise a generic registry or a
single migration format until it demonstrates a smaller change surface.
Benefit is long-term; complexity is very high; state should remain triage/planned.

### #223 Recovery and lifecycle

**Partial, with the restore assertion fixed.** A full restore path is now
present: authenticated/decompressed data is applied by
`apply_restored_backup` (`apps/aether-gateway/src/backup/mod.rs:38-88`), and
the `aether-backup-restore` binary verifies/decrypts input and writes private
output (`apps/aether-gateway/src/bin/aether-backup-restore.rs:113-143`). The
README documents its invocation (`README.md:165-168`). Relevant tests include
`authenticated_recovery_restores_password_and_api_key_login_material`
(`apps/aether-gateway/src/tests/control/admin/system_import.rs:1792`) and
`restore_rejects_zstd_output_over_limit`
(`apps/aether-gateway/src/backup/executor.rs:1261`). The claim that no restore
or import path exists is therefore false.

The DLQ remains intentionally unbounded at the append call
(`crates/aether-usage/runtime/src/queue.rs:251-263`), and this review found no
operator redrive surface. The transfer itself is more robust than the issue
states: atomic transfer has dedicated Redis concurrency tests, e.g.
`redis_dead_letter_transfer_concurrent_consumers_archive_exactly_once`
(`crates/aether-runtime/state/src/redis/dead_letter_transfer_tests.rs:196`).
Task retry configuration remains mostly one attempt
(`apps/aether-gateway/src/task_runtime/mod.rs:56-57,148-404`), and Postgres
migration checksum mismatch is still warning-only
(`crates/aether-data/adapters/postgres/src/migrations.rs:358-364`).

**Todo:** implement DLQ list/redrive with explicit idempotency and a configured
retention cap; add a restore-runbook round trip against each supported backend;
choose retries per task class rather than raising all retries. The DLQ task is
P1/M; the rest are P2 decisions. Acceptance must include duplicate-redrive,
retention and restore-apply integration tests.

### #224 Multi-node operation

**Partial.** Multi-node topology is actively validated, rather than merely
assumed: `validate_deployment_topology` is invoked from startup
(`apps/aether-gateway/src/main.rs:2064,2198`) and has tests such as
`multi_node_rejects_memory_runtime_backend`
(`apps/aether-gateway/src/main.rs:4775`). The premise that the topology is
unsupported is false.

The remaining operational gap is reproducibility: pressure tooling and
multi-node requirements are not presented as a deployable reference topology.
Redis outage behavior and per-process caches are architecture decisions, not
proof that all multi-node deployments are incorrect.

**Todo:** publish a three-node compose/reference deployment plus a capacity
exercise command; document Redis failure semantics and cache invalidation
window. Acceptance is a CI smoke boot with three identities and shared Redis.
Benefit high for operators, complexity medium; P1 docs/config work.

### #225 Documentation consistency

**Partial.** The reported tunnel names were corrected in the current README:
the documented Aether connect and DNS variables now match clap definitions
(`apps/aether-tunnel/README.md:220,229`; `apps/aether-tunnel/src/config.rs:311,423`).
The TCP keepalive entry is still worth checking against the current dirty
README change; source declares `AETHER_TUNNEL_TCP_KEEPALIVE`
(`apps/aether-tunnel/src/config.rs:635-637`). Do not carry the former
seven-variable claim forward without an automated table check.

Metrics, multi-node, and recovery documentation remain useful separate work;
backup restore has at least a command-level README entry, so describe the
remaining need as a runbook and drill, not "zero documentation".

**Todo:** generate or test the tunnel env table from clap metadata; add metrics
scrape, multi-node, and recovery-runbook documents. Acceptance: CI asserts all
documented env names resolve to a clap `env=` declaration and a restore drill
uses the public instructions. P1/S-M.

### #226 Dependencies and local components

**Partial.** `wreq` is still pinned to an RC and contains WebSocket support
(`Cargo.toml:139-140`), so upgrade/exit ownership is useful. The alleged
unintentional rustls default-feature inclusion needs a fresh dependency-tree
proof before a code change; the cited manifest line no longer contains that
dependency in this checkout. Do not schedule a speculative one-line fix.

**Todo:** capture `cargo tree -i aws-lc-rs` and an equivalent clean build
measurement, then choose pin/upgrade/isolation. Independently add formula
finite/overflow tests and share its allowed-function declaration with admin
validation. Acceptance: edge-case test matrix and an admin acceptance test for
every exported function. P2/S for formula work; P2/M for transport strategy.

### #229 Developer workflow

**Still valid, but small.** The issue identifies a duplicated allowed-function
contract; its remediation should be paired with #226 rather than migrated as
a broad naming cleanup. Architecture tests can be source-sensitive, so a
module map must include these checks rather than hide them.

**Todo:** export one billing function allowlist, use it from the admin parser,
and test equality; add a concise command/module map. Acceptance: adding a
function changes one declaration and both engine/admin tests pass. P2/S.

### #235 Maintainability governance

**Partial and documentation-led.** ADR/protocol/ownership concerns are valid
governance work, but line counts and commit-derived claims are snapshots, not
runtime defects. The current restore implementation and its tests show that
the codebase now has a recovery decision surface that should be documented.

**Todo:** create an ADR index, then write narrowly scoped ADRs for restore
atomicity, tunnel protocol version behavior, and usage retry/DLQ policy.
Acceptance is an approved status, links to code/tests, and a stated rollback
or compatibility rule. P2/M.

### #241 Architecture identity and consistency

**Partial.** `GatewayDataConfig::with_redis_url` still discards both inputs
(`apps/aether-gateway/src/data/config.rs:74-80`); this is a real misleading
API and should either be removed, implemented, or documented as a deprecated
compatibility no-op. It is not evidence that actual runtime coordination is
in-memory: multi-node startup validation is active (see #224).

Pre-reservation, cache invalidation, Redis-loss behavior, API versioning and
tunnel capability evolution are independent design choices. They must not be
combined into one unsafe refactor.

**Todo:** first remove/deprecate the no-op builder with a compile-time call-site
migration; then write ADRs for money admission and cache invalidation. Acceptance:
no silent no-op public builder and targeted multi-node tests. P2/S for the
builder, P1-M only after the financial policy is chosen.

### #247 Product behavior

**Partial.** Local balance denial still sends HTTP 429 while identifying itself
as `balance_exceeded` and passing `RateLimit` into response formatting
(`apps/aether-gateway/src/api/response.rs:246-281`). This is a concrete
client-contract ambiguity. The broad claim that notifications never dispatch
is too strong: notification dispatch infrastructure and items exist
(`apps/aether-gateway/src/important_notification.rs:165,265,496-508`), and
the provider-quota worker uses it. The question is which business transitions
are intentionally wired, not whether the subsystem is absent.

**Todo:** decide the documented balance status/code and retry headers, then add
OpenAI/Claude contract tests. Inventory notification producers and add only
the approved low-balance/refund events, each with preference and idempotency
tests. P1/S-M.

### #253 Financial and abuse controls

**Decision, not a proven "zero-control" defect.** Defaults do include a $10
initial gift and a disabled Turnstile switch (`crates/aether-admin/src/system.rs:2229,2256`),
but deployment defaults are policy knobs rather than proof that an operator
runs a public, unverified registration service. Wallet settlement explicitly
models overdraft (`crates/aether-data/contracts/src/repository/settlement/types.rs:444-502`)
and `insufficient_quota` is a terminal billing state
(`crates/aether-data/contracts/src/repository/usage/types.rs:2245`); the real
event chain must be measured before claiming delivered requests are free.

**Todo:** write a threat model for public registration, then choose hardened
production defaults, per-IP/account limits, gift/referral eligibility, and
the authorized overdraft policy. Add concurrent admit/settle simulations and
an `insufficient_quota` alert/report test before changing charging semantics.
Benefit potentially critical; complexity high and product-sensitive; keep as
P0 decision/planning rather than direct implementation.

### #254 Client API contract and documentation

**Partial.** The error-formatting helper is now used through multiple OpenAI
and execution paths (`apps/aether-gateway/src/handlers/public/ai_public.rs:1321`,
`apps/aether-gateway/src/executor/outcome.rs:753`), so the former assertion
that it applies only to Claude is stale. A `model_not_found` code is also
emitted by the public model response path
(`apps/aether-gateway/src/handlers/public/support/models/responses.rs:82`) and
tested (`apps/aether-gateway/src/tests/frontdoor/ai.rs:3107`).

Endpoint docs remain incomplete relative to the public surface. Keep docs and
error-contract work distinct, and consolidate the latter with #247.

**Todo:** add chat/images quickstarts and a compatibility/error matrix; ensure
each documented route has a request/response fixture. P1/S-M. Acceptance is a
doc test or fixture-driven contract test for listed routes.

### #255 Administrator experience and audit

**Partial.** Audit endpoints are not unauthenticated: operational permissions
require scoped admin access, and sensitive audit reads require a full admin
role (`apps/aether-gateway/src/api/ops.rs:214-246`); authorization tests live
in `apps/aether-gateway/src/tests/operational_auth.rs` (for example,
`downgraded_audit_admin_management_token_cannot_read_full_admin_audit_routes`).
The no-auth subclaim is false.

`emit_admin_audit` does create structured audit events, but its present
implementation writes them to tracing (`apps/aether-gateway/src/audit/admin.rs:30-110`).
The audit repository/table also supports query and lifecycle behavior. The
correct remaining question is whether all required **admin mutations** need a
durable record, and what must happen when that storage is unavailable; it is
not safe to infer from this function alone that every audit view is empty.

**Todo:** define an audit event schema and durability/failure policy, then add
the repository writer in the final response path with an integration test for
one successful and one rejected admin mutation. Keep upgrade backup/rollback
as a separate operations runbook task because release image and restore
facilities now exist. P0/M for audit policy+write path.

### #256 Contribution and fork governance

**Partial.** Current code still contains upstream install URLs
(`apps/aether-gateway/src/handlers/public/support/install.rs:23-25`), so the
fork-identity concern is concrete for that flow. The legal interpretation of
the licence is a maintainer/legal decision and must not be converted into code
without an approved policy.

**Todo:** add CONTRIBUTING and SECURITY with fork/upstream scope and supported
verification commands; replace fork-facing links only where the project
intends to distribute this fork's artifacts; add a CHANGELOG policy. Acceptance:
new contributor path has one canonical repository, disclosure route, and
working commands. P2/S.

## Status Changes Suggested

Close or annotate as superseded: #220 checksum and image-digest subclaims;
#223 no-restore subclaim; #224 topology-unsupported implication; #225
seven-name mismatch; #254 Claude-only formatter and missing model-not-found
subclaims; #255 unauthenticated audit route. Keep their remaining scoped work
as child tasks, so the issue tracker reports current behavior rather than
historical snapshots.
