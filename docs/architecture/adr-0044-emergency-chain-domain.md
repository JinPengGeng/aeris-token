# ADR-0044: Emergency chain domain boundary

Status: Proposed

## Context

Operations sometimes needs a deterministic, temporary failover chain. Reusing
`FixedOrder`, routing groups, names, or normal ranking would let emergency state
silently become ordinary multi-tenant scheduling state. It would also make the
order sensitive to priority, health, affinity, or a load-balancing seed.

## Decision

The scheduler core defines a separate `EmergencyChainGrant` capability:

- The grant ID is an opaque typed value. A grant is bound to one exact principal,
  an explicit non-wildcard operation set, and exactly one request/session scope.
  That immutable scope contains a server request ID, a canonical request SHA-256
  fingerprint, and an opaque one-time session nonce. A grant cannot be reused by
  another request during its TTL.
- Targets are exact `(provider_id, endpoint_id, key_id)` identities. The ordered
  target vector is private and immutable after issuance. Names are not accepted
  and there is no default or name fallback.
- A domain-separated, length-prefixed SHA-256 chain hash binds target identities
  and their order. URLs, credentials, provider names, and key names are outside
  the grant schema.
- Issuance requires a non-empty chain, a non-empty operation scope, unique
  operations and targets, and an expiry after issuance. The absolute TTL ceiling
  is 24 hours; callers may enforce a shorter policy.
- Revocation is monotonic. It cannot be cleared or moved later. A newly observed
  earlier effective instant may only tighten the revoked interval; repeated or
  later revocations are idempotent.
- Progress is owned by the grant domain object, not supplied as caller-authored
  `attempted_targets`. Authorizing a target reserves one non-cloneable permit
  and locks the grant. Only consuming that exact permit as a terminal outcome,
  retryable failure, or authoritative safe skip can update the cursor. Terminal
  outcomes consume the one-request grant; retrying the last slot exhausts it.
- A request that asks for emergency routing fails closed for a missing/mismatched
  grant, principal or operation overreach, request ID/fingerprint/nonce drift,
  future/expired/revoked grant, chain hash drift, outstanding permit, consumed or
  exhausted session, out-of-order target, or target outside the chain.
- Candidate matching returns one slot for every original chain position. A
  missing materialization is `candidate_index: None`; it is never compressed
  away. An available later slot remains unauthorized until the missing earlier
  slot receives an authoritative safe-skip permit completion. Duplicate
  candidate identities are ambiguous and fail closed.

## Trusted input boundary

`NormalRouting` requires `ServerNormalRoutingActivation`, which may only be
created by server route selection. Emergency principal and operation come from
authenticated server context and server route selection. The request scope and
chain hash come from server grant state. Gate and completion instants come from
the server clock. These types intentionally have no serde deserialization
contract.

No adapter may populate these values from headers, JSON bodies, query strings,
cookies, client timestamps, or any other client-controlled field. Client input
may identify an ordinary request, but cannot activate emergency mode, select its
principal/operation, assert progress, or choose the gate clock.

## Gate time semantics

The `ServerEmergencyChainInstant` inside trusted context is the linearization
instant for one gate evaluation. Every predicate is evaluated against that same
value. Validity is the half-open range `[issued_at, expires_at)`, and a
revocation is effective when `revoked_at <= gate_at`.

The in-memory permit is not a durable authorization lease. Future integration
must strong-read grant/session state and atomically reserve its permit with CAS
at the send boundary. Completion, including `AuthoritativeSafeSkip`, must be
derived from the authoritative attempt/materialization ledger and persisted by
CAS. A client assertion or a locally compressed candidate list is never proof
that an earlier slot was attempted or safely skipped.

## Deferred integration

This slice intentionally does not define administrator permissions, storage,
audit persistence, admin HTTP routes, gateway wiring, or send-time strong-read
transactions. Those controls must be added before the capability is enabled.

## Consequences

Emergency routing cannot inherit normal scheduler ranking, silently omit a
missing slot, or fall back to a target outside the grant. The domain slice makes
the required invariants explicit, but the capability must remain disabled until
the deferred authoritative ledger, CAS reservation/completion, audit, and
send-time strong-read integration exists.
