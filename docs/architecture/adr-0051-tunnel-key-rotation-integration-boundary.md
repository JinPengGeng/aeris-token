# ADR-0051: Tunnel signing-key rotation integration boundary

## Status

Proposed follow-up to #205-A and PR #365. This document deliberately does not
change the tunnel wire protocol or database schema.

## Evidence

PR #365 adds `aether_contracts::tunnel_key_rotation::TunnelSigningKeySet`, an
in-memory contract with validity windows, overlap, explicit rollback, and
revocation. A repository search at `origin/main` (2026-09-13) found no gateway
consumer of this type, no persistence model/migration for signing keys, and no
wire field carrying a tunnel release/signing key id. Existing `key_id` fields
belong to provider/API-key routing and must not be reused for tunnel identity.

## Decision

Keep the contract layer independent until the following protocol decisions are
approved in a separate issue/PR:

1. **Persistence owner:** gateway control-plane database table and migration,
   including encrypted key material ownership, active-key selection, and audit
   events. Secrets must remain in the deployment secret store; persistence may
   hold ids and validity metadata only.
2. **Wire location:** a versioned tunnel registration/handshake field (or
   authenticated header) carrying `tunnel_signing_key_id`. It must be covered by
   the existing authenticated transcript and reject duplicates/unknown ids.
3. **Compatibility:** old clients without the field remain accepted only during
   an explicitly bounded migration window; after the cutover, missing ids fail
   closed. The gateway must never infer a key from provider `key_id`.
4. **Rotation operations:** overlap, expiry, revoke, rollback, and recovery
   commands need owner, authorization, audit, and observable failure semantics.

## Acceptance tests for the follow-up

- migration/read-back test proves active and overlapping keys survive restart;
- gateway registration test proves the exact wire key id is persisted and
  authenticated, while missing/unknown/expired/revoked ids fail closed;
- old/new client compatibility test exercises the bounded migration window;
- rotation runbook test proves rollback selects only a still-valid key and never
  logs secret material.

Until those tests exist, PR #365's contract tests are necessary but not
sufficient evidence for closing #205. No runtime gateway or tunnel changes are
included in this slice.

## Rollback

Revert the documentation-only PR. No runtime behavior or schema is changed.
