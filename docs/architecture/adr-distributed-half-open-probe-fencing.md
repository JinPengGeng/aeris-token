# ADR: Distributed HalfOpen probe fencing contract

Status: Accepted and production-wired

Issue: #52

## Context

HalfOpen probes are scoped by `(provider_key_id, api_format)`. A process-local
mutex cannot prevent two gateway instances from probing the same scope. A lease
alone is also insufficient: a paused owner can resume after its lease expires
and overwrite a newer result.

The Redis lock counter is not authoritative because Redis persistence may be
disabled or its namespace may be reset. The provider-key circuit JSON is the
durable scheduling record and already supports compare-and-swap updates.

## Decision

1. Distributed probe ownership uses the shared Redis `RuntimeState` lock lane.
   The typed coordinator rejects the memory backend during construction.
2. A Redis claim contains its scope, owner, opaque owner token, absolute
   client-side expiry, TTL, and a Redis-local diagnostic fencing token.
   The Redis token is never the authoritative cross-restart fence.
3. Acquisition, renewal, release, and CAS errors are returned to the caller.
   Callers must treat errors and false results as `Stop`; there is no local
   fallback.
4. After Redis acquisition, admission CASes a durable claim into the exact
   provider-key/API-format circuit. Its fence is the previous durable circuit
   fence plus one. An unexpired durable claim denies admission even if Redis was
   restarted or flushed.
   The initial lease and durable-claim TTL cover the plan's maximum transport
   timeout plus a completion margin. While a probe remains active, the gateway
   renews both records. If either renewal fails, it writes a non-expiring
   durable isolation claim where possible and refuses further sends for that
   scope; recovery then requires an explicit operator repair.
5. Completion validates and extends the exact Redis lease without releasing it.
   The health/circuit CAS then commits the terminal projection together with a
   serialized `half_open_completion_pending` outbox marker. The marker is
   written to the SQL completion audit and cleared by a second circuit CAS;
   only then is the Redis lease token-CAS released. A later admission replays a
   marker left by a crash or SQL error before making its send decision.
6. PostgreSQL, MySQL, and SQLite provide the monotonic SQL implementation. The
   memory completion repository always returns an invalid-configuration error.

## Ordering and failure behavior

The required sequence is:

```text
final catalog read -> try_acquire -> durable claim CAS -> upstream send(s) for one candidate
                   -> authorize_completion/renew -> health + pending-marker CAS
                   -> complete_if_newer -> pending-marker CAS clear -> release
```

- If the lease expires or is lost before completion authorization, no SQL audit
  write or explicit release is authorized.
- The lease is not deleted before the circuit CAS or completion audit, so a
  second owner cannot enter an immediate re-probe window.
- If Redis restarts, the durable claim still denies contenders until expiry.
  The next claim increments the durable circuit fence, independent of the reset
  Redis counter.
- SQL errors are fail closed. The pending marker retains the exact completion
  value for replay; a concurrent higher fence still wins.
- Before any SQL write, the pending marker must validate as belonging to the
  provider key and API format that carry it, and its fence must equal the
  circuit's current durable fence. Malformed, cross-scope, and stale-fence
  markers fail closed. After marker cleanup, the provider key is reloaded and
  the marker must be observed absent before the lease can be released.
- A release is not a consume and never creates completion authority.

## Operator repair runbook

Normal health recovery deliberately preserves an active or isolated durable
claim and any pending completion. It must not be used to clear isolation.

To repair a claim whose `expires_at_unix_ms` and circuit
`half_open_until_unix_ms` are both `u64::MAX`, first verify that no pending
completion exists and record the claim's exact owner and durable fence. Then
call:

```text
PATCH /api/admin/endpoints/health/keys/{key_id}/half-open-isolation
  ?api_format={api_format}
  &expected_fence={fence}
  &expected_owner={owner}
```

The repair uses the complete provider-key health snapshot as its CAS
expectation. It rejects a missing/malformed claim, a changed owner or fence, a
non-isolated claim, or any pending completion. Success removes only the claim
and the non-expiring isolation timestamp; it preserves the durable fence and
the circuit's open state. The response reports the actual open state and does
not claim that ordinary circuit recovery occurred.

## Key construction

The Redis coordination key hashes a versioned, length-framed provider-key and
API-format tuple. This avoids delimiter ambiguity and does not expose provider
identifiers in Redis key names.

## Consequences

- Multi-instance deployments cannot silently use memory coordination.
- Durable completion order is independent of wall-clock order.
- Redis and SQL are not placed in a distributed transaction. Crash consistency
  comes from the replayable pending marker, not from atomic cross-store commit.
- The normal sync candidate gate precedes Grok, ChatGPT-Web image, Windsurf, and
  direct/tunnel/remote execution. The normal stream candidate gate precedes
  Grok, Windsurf, Kiro web-search, ChatGPT-Web image, and
  direct/tunnel/remote execution. Standalone transport and admin/model-test
  entrypoints use the same typed gate and apply terminal health effects before
  returning. Contention and coordination failures produce no upstream request.
- An OAuth retry within the same logical request/candidate/key/format reuses the
  existing admitted session; it cannot acquire a second claim or self-contend.
- Health projections preserve both the durable fence and unrelated active
  claims. Only a completion authorized by the matching durable fence removes
  its claim as part of the success/failure circuit transition.
- Every admitted frame-stream transport transfers one terminal guard through
  first-frame parsing into the background pump. An early parse error, dropped
  response, missing terminal event, or aborted pump drops that guard and
  isolates any still-active durable claim.
