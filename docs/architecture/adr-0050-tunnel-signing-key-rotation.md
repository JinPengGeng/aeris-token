# ADR-0050: Tunnel signing key rotation overlap

## Decision

Tunnel handshake signing keys have an explicit `key_id`, `not_before`, and
exclusive `expires_at` window. During rotation the verifier accepts both the
old and new IDs for the configured overlap interval. The signer uses only the
configured active (new) key. Verification selects the key by the supplied ID;
it never tries another key, so expired or revoked IDs cannot select a key.

Rollback is an operator change of the active key to a still-valid key. A key
whose validity window has not started cannot sign, and a revoked key is removed
from verification immediately. The contract is implemented by
`aether_contracts::tunnel_key_rotation::TunnelSigningKeySet`. Duplicate IDs and
empty key material are rejected when constructing a set; configuration order
must never silently select a different secret or validity window for one ID.

Persistence and wire integration remain follow-up work. The wire key ID must
be authenticated; this in-memory contract alone does not establish transcript
compatibility, prevent replay, or complete the gateway integration.

Key material is an opaque secret owned by the deployment secret store. The
contracts type keeps it private and its `Debug` implementation emits only
`<redacted>`; callers must not log or serialize `key_material()`. IDs and
validity metadata may be persisted separately from the secret.
