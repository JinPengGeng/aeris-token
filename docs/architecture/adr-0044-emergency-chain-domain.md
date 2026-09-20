# ADR-0044: Emergency chain domain boundary

Status: Accepted (Issue #44 administrator operations v1; scheduler scaffold deferred)

This decision records two deliberately separate implementations: the sealed
scheduler-domain scaffold, and a narrow synchronous administrator model-test
route backed by a persisted one-shot grant. The administrator route does not
claim to integrate the scheduler scaffold's opaque permit or unavailable
ledger authority. That scaffold is retained as a possible design for a broader
scheduler feature, not as an Issue #44 acceptance requirement.

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
  and locks the grant. The permit has no target getter, send method, completion
  method, or conversion to an attempt receipt. This slice therefore cannot
  claim that invoking a closure equals one physical upstream send and cannot
  advance based on caller-selected `Ok`/`Err` outcomes. The only implemented
  transition consumes the permit with an opaque ledger safe-skip proof.
- A request that asks for emergency routing fails closed for a missing/mismatched
  grant, principal or operation overreach, request ID/fingerprint/nonce drift,
  future/expired/revoked grant, chain hash drift, outstanding permit, consumed or
  exhausted session, out-of-order target, or target outside the chain.
- Candidate matching returns one slot for every original chain position. A
  missing materialization is `candidate_index: None`; it is never compressed
  away. An available later slot remains unauthorized until the missing earlier
  slot receives an authoritative safe-skip permit completion. Multiple
  candidates matching the same grant target are ambiguous and fail closed;
  candidates outside the grant are ignored.

## Trusted input boundary

Route/auth/time values are minted through `GatewayEmergencyChainAuthority`.
`EmergencyChainLedgerAuthority` is separately sealed, but it deliberately
returns `AuthoritativeLedgerUnavailable` for every production safe-skip proof
request. A non-empty ledger-entry string is not authority evidence and cannot
unlock progress. The test-only fixture used by this domain module is not
compiled into production artifacts. If the deferred scheduler design is
implemented, its ledger integration must verify a committed record for the exact
request scope, chain hash, slot, permit/fencing identity, authorization instant,
and observation instant before it can mint a proof. Both authorities have no
safe public constructor, `Default`, `Clone`, or serde contract; ordinary
dependency crates cannot forge them, and gateway dispatch authority cannot mint
ledger proof. This preparatory slice deliberately exposes no bootstrap path, so
the scheduler capability remains unusable until narrowly owned authority
bootstraps are reviewed and added.

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

## Administrator operations v1

The production Gateway exposes a separate, intentionally small operations
path under `admin:provider_query`:

- `POST /api/admin/provider-query/emergency-chain/execute` accepts one model,
  one provider ID, and 1 to 32 explicitly ordered endpoint/key ID pairs.
- `POST /api/admin/provider-query/emergency-chain/{grant_id}/revoke` revokes a
  grant only for the administrator principal that owns it.
- Both routes require an authenticated administrator principal and the
  `admin:provider_query:admin` management-token permission when a management
  token is used.
- The server generates grant, request, nonce, fingerprint, hash, and time
  fields. Grants have a fixed five-minute TTL and persist only IDs; credentials,
  URLs, names, and caller timestamps are not grant fields.
- Issuance and its audit record commit atomically. The grant is then consumed
  once in a row-locking transaction before any upstream send. A crash after
  consumption leaves the grant consumed; the Gateway never replays it.
- The Gateway preserves the submitted target order and does not invoke normal
  scheduler sorting, ranking, or fallback. It strong-reads the grant and the
  current provider, endpoint, key, and uncached transport before every send.
- v1 permits only synchronous text model-test formats. It does not accept
  streaming or tools. It advances only after an explicit transient HTTP model
  test failure (`408`, `429`, or `5xx`); success and all other outcomes stop the
  chain.

This route is an operations-only escape hatch. It is not a tenant request
router, a generalized attempt ledger, or a replacement for the normal
scheduler.

## Issue #44 acceptance boundary

Issue #44 asks for a request-scoped immutable failover chain for operations,
with authorization, audit, bounded expiry, rollback, and no promotion into the
default multi-tenant scheduler. The administrator operations v1 route is the
accepted implementation of that scope: server-generated request identity and
hashes bind an immutable ordered target list; administrator authorization and
management-token permission protect issuance and revocation; issuance is
atomically audited; the fixed five-minute expiry is rechecked before sends; and
owner-bound revocation plus one-shot consumption provide rollback and replay
protection. The separate route never participates in normal scheduler ordering
or fallback.

The opaque permit, authoritative attempt ledger, and versioned CAS send boundary
below are deferred scheduler hardening for a separately approved public or
tenant routing feature. They are not prerequisites for accepting or closing
Issue #44's operations-only scope.

## Gate time semantics

The `ServerEmergencyChainInstant` inside trusted context is the linearization
instant for one gate evaluation. Every predicate is evaluated against that same
value. Validity is the half-open range `[issued_at, expires_at)`, and a
revocation is effective when `revoked_at <= gate_at`.

The implemented gate only reserves an in-memory permit. It deliberately exposes
no domain send or completion path: a `FnOnce` closure call cannot prove that a
physical upstream write happened, and returning `Ok` or `Err` cannot be treated
as an authoritative attempt outcome.

If a future design integrates emergency routing into public or tenant
scheduling, or requires cross-instance proof of exactly one physical send, it
must add a gateway-owned dispatch port that consumes the opaque permit and, at
the actual send boundary, obtains a fresh server time and live request context,
strong-reads expiry/revocation/session version, rechecks all gate predicates,
and wins a versioned CAS before revealing the target and performing exactly one
physical send. A stale instance or CAS loser must never receive a send
capability. No progress receipt may be created merely because a closure
returned. Completion must be derived from the authoritative attempt ledger and
committed with CAS. Safe skip uses
`EmergencyChainSafeSkipProof`, minted only by the ledger authority after an
authoritative materialization-ledger record is committed, and its transition
must use the same CAS discipline. A client assertion or locally compressed
candidate list is never proof of attempt or safe skip.

## Boundary between the two implementations

The scheduler-domain permit remains unusable in production because
`EmergencyChainLedgerAuthority` still cannot mint authoritative completion or
safe-skip proof. The administrator v1 route does not construct or unwrap that
permit. Its persisted one-shot consumption prevents replay, while its
strong-read-before-send checks provide the narrower operations contract above.
Public or tenant emergency scheduling is explicitly outside Issue #44. If that
broader feature is separately approved, it must first adopt an authoritative
attempt ledger and CAS send boundary such as the deferred design described here.

## Consequences

The administrator model-test path can execute a short-lived fixed chain without
inheriting normal scheduler ranking or falling back outside the grant. This
satisfies Issue #44's operations-only boundary. Public and tenant emergency
routing remains disabled and requires a separate decision; the opaque permit,
ledger, and CAS scaffold remains available as a deferred design for that work.
