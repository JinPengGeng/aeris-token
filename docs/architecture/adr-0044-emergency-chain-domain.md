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
  and locks the grant. The permit has no target getter. It must be consumed by a
  send-boundary revalidation to produce a non-cloneable dispatch capability,
  whose `FnOnce` dispatch consumes it. Only the resulting dispatched-attempt
  receipt can record success/failure. A safe skip instead consumes the permit
  with an opaque ledger proof. Terminal outcomes consume the one-request grant;
  retrying the last slot exhausts it.
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

All trusted values are minted through `GatewayEmergencyChainAuthority`. This is
a sealed capability with no safe public constructor, `Default`, `Clone`, or
serde contract; ordinary dependency crates cannot forge it. This preparatory
slice deliberately exposes no bootstrap path, so the capability remains
unusable until a gateway-owned authority bootstrap is reviewed and added.

`NormalRouting` requires `ServerNormalRoutingActivation`. Emergency principal
and operation come from
`AuthenticatedEmergencyChainPrincipal` and
`ServerSelectedEmergencyChainOperation`. Grant ID and chain hash come from
`ServerEmergencyChainGrantActivation`, built from server grant state. In
contrast, `LiveEmergencyChainRequestContext` is independently derived from the
currently authenticated request and is compared with the stored grant scope;
it must never be copied from the grant to make the comparison tautological.
Gate and completion instants come from the server clock.

No adapter may populate these values from headers, JSON bodies, query strings,
cookies, client timestamps, or any other client-controlled field. Client input
may identify an ordinary request, but cannot activate emergency mode, select its
principal/operation, assert progress, or choose the gate clock.

## Gate time semantics

The `ServerEmergencyChainInstant` inside trusted context is the linearization
instant for one gate evaluation. Every predicate is evaluated against that same
value. Validity is the half-open range `[issued_at, expires_at)`, and a
revocation is effective when `revoked_at <= gate_at`.

The initial gate only reserves an in-memory permit. `linearize_dispatch` takes a
fresh trusted live-request context, rechecks grant identity, request scope,
principal, operation, hash, expiry and revocation, consumes the permit, and
marks the session dispatching. `dispatch_once` then consumes the resulting
capability and exposes the target only to one `FnOnce` send closure.

This still is not a distributed authorization lease. Future integration must
strong-read grant/session state and persist the `Ready -> Outstanding ->
Dispatching -> Ready/Consumed/Exhausted` transition with versioned CAS. A stale
instance or CAS loser must not receive or use a dispatch capability. Dispatch
completion must use the authoritative attempt ledger. Safe skip must use
`EmergencyChainSafeSkipProof`, minted only by the gateway authority after an
authoritative materialization-ledger record is committed, and its transition
must use the same CAS discipline. A client assertion or locally compressed
candidate list is never proof of attempt or safe skip.

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
