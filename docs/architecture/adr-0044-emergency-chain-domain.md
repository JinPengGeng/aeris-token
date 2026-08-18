# ADR-0044: Emergency chain domain boundary

Status: Proposed

## Context

Operations sometimes needs a deterministic, temporary failover chain. Reusing
`FixedOrder`, routing groups, names, or normal ranking would let emergency state
silently become ordinary multi-tenant scheduling state. It would also make the
order sensitive to priority, health, affinity, or a load-balancing seed.

## Decision

The scheduler core defines a separate `EmergencyChainGrant` capability:

- The grant ID is an opaque typed value. A grant is bound to one exact principal
  and an explicit, non-wildcard operation set.
- Targets are exact `(provider_id, endpoint_id, key_id)` identities. The ordered
  target vector is private and immutable after issuance. Names are not accepted
  and there is no default or name fallback.
- A domain-separated, length-prefixed SHA-256 chain hash binds target identities
  and their order. URLs, credentials, provider names, and key names are outside
  the grant schema.
- Issuance requires a non-empty chain, a non-empty operation scope, unique
  operations and targets, and an expiry after issuance. The absolute TTL ceiling
  is 24 hours; callers may enforce a shorter policy.
- Revocation is monotonic. Once a grant has a revocation instant it cannot be
  cleared or moved by this API.
- A request without an emergency grant follows normal routing unchanged. A
  request that asks for emergency routing fails closed for a missing/mismatched
  grant, principal or operation overreach, future/expired/revoked grant, chain
  hash drift, a non-prefix attempt history, an out-of-order target, or a target
  outside the chain.
- Candidate matching emits only exact chain identities and preserves chain
  order. It does not inspect priority, health, affinity, ranking mode, or seed.
  Duplicate candidate identities are ambiguous and fail closed.

## Gate time semantics

`gate_at_unix_secs` is the linearization instant for one pure evaluation. Every
predicate is evaluated against that same value. Validity is the half-open range
`[issued_at, expires_at)`, and a revocation is effective when
`revoked_at <= gate_at`.

The decision is not a durable authorization lease. The future integration must
strong-read the grant and evaluate the gate again at the send boundary. An
attempt prefix passed to the pure API is only domain input; persistence must
prove it from authoritative attempt records before sending.

## Deferred integration

This slice intentionally does not define administrator permissions, storage,
audit persistence, admin HTTP routes, gateway wiring, or send-time strong-read
transactions. Those controls must be added before the capability is enabled.

## Consequences

Emergency routing cannot inherit normal scheduler ranking or silently fall back
to a target outside the grant. Callers must carry the opaque ID, exact principal,
operation, chain hash, and authoritative attempt prefix, and must re-authorize at
the eventual mutation boundary.
