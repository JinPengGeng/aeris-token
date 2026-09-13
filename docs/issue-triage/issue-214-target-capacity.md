# Issue #214: automatic per-target admission capacity

Decision: implement this P1 capacity slice; keep #214 open for admission
lifetime and fault-injection acceptance. Revalidated against fork main
`b044b1292694c1850a6efb7eccc9da63451be7bd`. Complexity and risk are medium.

## Accuracy and benefit

The global HTTP request limit and the per-target limit used independent
CPU/file-descriptor formulas. With four CPUs and a 65,536 FD limit, the actual
global request capacity is 4,096 while the automatic target limit is 10,000.
An explicitly smaller global request limit did not constrain target admission.
The older audit's description of two equal 10,000 floors was outdated, but
the lack of useful default isolation remained real.

For automatic target configuration, bind per-process target admission to
`max(1, min(existing_auto, effective_global / 4))` when the global request
capacity is assembled. This keeps the existing FD bound and leaves room for
other targets while one target holds its admitted permits. The fraction is a
conservative default, not a workload-specific throughput guarantee. Target
queueing retains its existing bounded timeout; queued attempts can briefly
hold global permits before rejection. A global capacity of one cannot isolate
targets and retains one usable permit.
The global capacity here is this process's HTTP request gate; optional shared
distributed limits remain separate and this does not reserve cluster-wide
capacity for each target.

Explicit positive target values remain operator overrides, including values
above the global cap; off/0 keeps target admission disabled. Unset, empty,
`auto` and invalid inputs retain automatic behavior through the same parser.
The configuration retains its original CPU/FD automatic ceiling, so repeated
builder configuration does not repeatedly divide an already reduced limit.
Embedded callers without a global request gate retain the existing target
ceiling; there is no effective global capacity to divide in that case.

## Resource consumers and scope

The two transport client-shard calculations use the configured CPU/FD ceiling
as a bounded connection-pool sizing hint. They are process-wide client-cache
settings and are not per-AppState traffic-admission quotas. Keep their existing
fixed/off/default interpretation and explicit shard overrides; do not publish
one AppState's effective capacity through process-global mutable state. The
actual per-target admission metric reports the effective reduced limit. Extra
client shards cannot bypass admission. The existing shard tests remain part of
validation. This distinguishes pool sizing from the number of admitted requests
without changing transport pooling or cache identity.

This is not complete lifetime isolation: direct inline streaming currently
releases its target permit at first client yield, while another stream path
holds it through upstream completion. Local tunnel and synchronous paths also
need a separate coverage decision. Those findings remain under #214; changing
only the default cannot prove slow post-first-byte or sync-request isolation.
No HTTP response contract, target identity, permit lifetime, or schema changes
are included here.

## Validation and rollback

Required focused coverage: CPU/FD/global-limit edge cases, explicit overrides,
the global=1 boundary, actual target A saturation while target B acquires
global and target permits, normal release, and cancellation restoring both
global and target capacity. The production AppState builder is exercised;
formula-only tests are insufficient. Run the existing target, configuration,
transport shard and request-admission tests, plus formatting and diff checks.
The final test results and protected CI links belong in the PR before merge.

Local Rust 1.95.0 results: the `target_` group passed 51 tests, including all
four new capacity/override/permit tests and the existing HTTP/1 and HTTP/2 shard
tests. The `gate_limit` group passed five tests and `request_admission` passed
the heartbeat/disconnect admission test. All ran against the same reviewed
gateway test executable with `RUST_MIN_STACK=16777216`. Formatting and diff
checks passed. Independent code review and the complete protected CI remain
required before merge.

Operators with a measured need for more single-target concurrency can set
`AETHER_GATEWAY_UPSTREAM_TARGET_GATE_LIMIT` to an explicit positive limit;
this gives up the automatic reserve. Rollback is a code revert or that documented
explicit override during the normal deployment process. No data migration is
needed. Retain #214's remaining readiness (#308), Redis error/restore,
SQL live-timeout and admission-lifetime acceptance until independently proven.
