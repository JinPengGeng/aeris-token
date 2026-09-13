# ADR-0050: Tunnel signing key rotation overlap

## Decision

Tunnel handshake signing keys have an explicit `key_id`, `not_before`, and
exclusive `expires_at` window. During rotation the verifier accepts both the
old and new IDs for the configured overlap interval. The signer uses only the
configured active (new) key. Verification selects the key by the supplied ID;
it never tries another key, so an expired or revoked key cannot be replayed.

Rollback is an operator change of the active key to a still-valid key. A key
whose validity window has not started cannot sign, and a revoked key is removed
from verification immediately. The contract is implemented by
`aether_contracts::tunnel_key_rotation::TunnelSigningKeySet`; persistence and
wire header plumbing can adopt it without changing the HMAC transcript.

Key material is an opaque secret owned by the deployment secret store. The
contracts type keeps it private and its `Debug` implementation emits only
`<redacted>`; callers must not log or serialize `key_material()`. IDs and
validity metadata may be persisted separately from the secret.
