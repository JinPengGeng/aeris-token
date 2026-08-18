# ADR: Distributed HalfOpen probe fencing contract

Status: Accepted as an inactive foundation slice

Issue: #52

## Context

HalfOpen probes are scoped by `(provider_key_id, api_format)`. A process-local
mutex cannot prevent two gateway instances from probing the same scope. A lease
alone is also insufficient: a paused owner can resume after its lease expires
and overwrite a newer result.

The current pool-score write path performs read/modify/write updates and has no
atomic predicate that can safely combine Redis ownership with SQL state. This
slice therefore adds a separate typed contract and durable fenced completion
row without enabling the production probe path or claiming that pool scores are
already protected.

## Decision

1. Distributed probe ownership uses the shared Redis `RuntimeState` lock lane.
   The typed coordinator rejects the memory backend during construction.
2. A claim contains its scope, owner, opaque owner token, absolute client-side
   expiry, TTL, and a Redis-generated monotonically increasing fencing token.
3. Acquisition, renewal, release, and consume errors are returned to the caller.
   Callers must treat errors and false results as `Stop`; there is no local
   fallback.
4. Completion requires consuming the exact live Redis token first. Redis expiry,
   owner replacement, or token conflict returns no completion permit.
5. The consumed permit creates a completion carrying the same scope, owner, and
   fence. SQL stores it only when the fence is strictly greater than the current
   fence for that scope.
6. PostgreSQL, MySQL, and SQLite provide the monotonic SQL implementation. The
   memory completion repository always returns an invalid-configuration error.

## Ordering and failure behavior

The required sequence is:

```text
try_acquire -> probe/renew -> consume_for_completion -> complete_if_newer
```

- If the lease expires or is lost before consume, no SQL write is authorized.
- If the process crashes after consume but before SQL, no completion is written.
  This loses availability for that attempt but preserves safety.
- A new owner may acquire immediately after consume. If its higher fence reaches
  SQL first, the older completion is rejected. If the older completion reaches
  SQL first, the higher fence can still supersede it.
- SQL errors are fail closed. A caller may retry the same completion value;
  a concurrent higher fence still wins.
- A release is not a consume and never creates completion authority.

## Key construction

The Redis coordination key hashes a versioned, length-framed provider-key and
API-format tuple. This avoids delimiter ambiguity and does not expose provider
identifiers in Redis key names.

## Consequences

- Multi-instance deployments cannot silently use memory coordination.
- Durable completion order is independent of wall-clock order.
- Redis and SQL are not placed in a distributed transaction. Consuming before
  SQL plus a strict SQL fence is the intentional fail-closed protocol.
- This slice does not update `pool_member_scores`, select candidates, start
  probes, or enable any feature flag. Production activation must explicitly wire
  the typed sequence and translate an accepted completion into scheduler state
  under a separately reviewed atomic contract.
