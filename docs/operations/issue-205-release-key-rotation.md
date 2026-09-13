# Tunnel release signing-key rotation

Refs #205. Implements the public trust-set overlap in
[ADR-0045](../architecture/adr-0045-signed-tunnel-release-provenance.md).
This is the Ed25519 signature on the exact bytes of `SHA256SUMS.txt` used by
manual and heartbeat-triggered self-upgrades. It does not change gateway
authentication, tunnel handshakes, database state, or the version-1 envelope.

## Build inputs and custody

`AETHER_TUNNEL_RELEASE_TRUST_KEYS` is a **build-time public** JSON array:

```json
[
  {"key_id":"release-old","public_key":"<base64 of 32 raw Ed25519 public-key bytes>"},
  {"key_id":"release-new","public_key":"<base64 of 32 raw Ed25519 public-key bytes>"}
]
```

Use protected repository variables for these public inputs. The signing private
key stays in the protected release secret `AETHER_TUNNEL_RELEASE_PRIVATE_KEY_PEM`;
do not put it in JSON, a repository variable, command arguments, workflow output,
logs, or uploaded artifacts. Authorize actual release jobs using the deployment's
release approval policy before publishing. This change does not provision keys,
alter repository protection, or publish a release.

| Input | Meaning |
| --- | --- |
| `AETHER_TUNNEL_RELEASE_TRUST_KEYS` | Complete authoritative set embedded in the binary; 1–16 public keys, at most 16 KiB of JSON. |
| `AETHER_TUNNEL_RELEASE_KEY_ID` | Active manifest signer. Required for release builds and publication; must belong to the set. |
| `AETHER_TUNNEL_RELEASE_PUBLIC_KEY` | Backward-compatible single-key input. When a set is present, leave empty or set to the active signer's exact public key. A conflict is rejected. |

An absent or empty trust-set variable preserves the existing `KEY_ID` plus
`PUBLIC_KEY` pair. With a nonempty set, no legacy key is added implicitly.
The parser rejects duplicate IDs, duplicate public keys under different IDs,
duplicate/unknown JSON fields, malformed base64, non-32-byte or weak Ed25519
keys, empty arrays, oversized sets, and IDs outside `[A-Za-z0-9._-]{1,128}`.
Whitespace-only input is malformed; the empty string means an unset GitHub
variable. A malformed set never falls back to the legacy key.

Tagged-release preflight and every native/cross build validate these inputs with
the production parser. Cross builds explicitly forward only the three public
inputs. The publication verifier uses that same parser and signature verifier,
and additionally requires the signed envelope's ID to equal the active signer.
`nightly.yml` and `release.yml` currently build the gateway only; they are not
alternate tunnel release paths.

The deployed tunnel reads these values with `option_env!`: runtime environment,
gateway instructions, and release metadata cannot add or remove trust. A manual
source build may omit every input; its self-upgrader then fails closed. For
source builds outside the workflow, validate before compiling:

```sh
cargo run --quiet --locked --manifest-path tools/ci/tunnel-release-verifier/Cargo.toml -- check
cargo build --release --locked -p aether-tunnel
```

Load public values from reviewed local configuration before those commands.
The helper needs Rust and compiles the production verifier with a small isolated
dependency graph; it never reads the signing private key.

## Record and verify public fingerprints

Before each stage, record the source commit, tag/version, key ID, and SHA-256
fingerprint of the **32 raw public bytes** in an independently reviewed release
record. JSON order is immaterial. Never reuse an old ID for a different key.
Given a reviewed public PEM file, the following exports only public material:

```sh
openssl pkey -pubin -in release-public.pem -outform DER -out release-public.der
test "$(wc -c < release-public.der)" -eq 44
dd if=release-public.der of=release-public.raw bs=1 skip=12 count=32
openssl dgst -sha256 release-public.raw
openssl base64 -A -in release-public.raw
```

Confirm the file is an Ed25519 public key, and have a second operator compare the
fingerprint with the approved key-custody record. The base64 output populates
`public_key`; the fingerprint alone cannot serve as a verification key. No
production fingerprint is asserted by the test fixtures or this document.

## Staged normal rotation

Choose strictly increasing tunnel versions B (bridge), S (signer switch), and R
(retirement). Keep the verified B artifact and its old-signed manifest available
for stragglers, together with the approved fingerprints and source commit.

| Stage | Embedded set | Manifest signer | Exit condition |
| --- | --- | --- | --- |
| Existing release | old | old | New public key reviewed independently. |
| B: distribute overlap | old + new | old | Supported nodes have installed B or later and tested new-key verification offline. |
| S: switch signing | old + new | new | Keep the overlap for at least 30 days after S and confirm the supported fleet has upgraded. Extend it while any supported/offline node still needs B. |
| R: retire old trust | new | new | Publish a new binary with old absent; retain controlled recovery artifacts. |

1. Add the reviewed new public key to `TRUST_KEYS`, keep `KEY_ID=old`, and retain
   the old private signer. Leave the legacy public variable empty or matching
   old. Validate and publish B under the normal approved release procedure.
   An old-only installed client can authenticate B because B is signed by old;
   its new executable then trusts both keys.
2. Verify B distribution and the new-key offline fixture for each supported
   platform. Change `KEY_ID` and the protected private signer to new together.
   Clear or update the legacy public variable, and keep both keys in the set.
   Publish S. The post-signing verifier blocks a mismatched ID/private key and
   any manifest mutation. Merely changing repository variables does not change
   an already installed binary.
3. After the window and fleet check, remove old from the set and publish R signed
   by new. An R binary rejects old signatures. Older overlap binaries still
   trust old until replaced; removing a variable is not emergency revocation.

An old-only client that skipped B cannot authenticate S or R. Route it through
the retained, independently verified bridge or a manual verified reinstall.
Do not globally switch back to old after any R clients exist: those clients
correctly reject it. Automatic downgrade/version checks remain in force.

## Failure recovery and emergency revocation

An unknown ID, invalid signature, or malformed trust configuration aborts before
archive extraction or replacement. Keep the current executable running and
record only the version, expected ID, and error class. Never add a public key
because an untrusted release envelope asks for it.

- Before S, a failed bridge rollout can continue using old while the reviewed
  build configuration is corrected. Publish a corrected higher version.
- Between S and R, a signer rollback is possible only after confirming every
  supported client still trusts old and old remains uncompromised. Keep both
  public keys and publish a higher version; do not rewrite an existing tag.
- After retirement, recovery must use new or an out-of-band verified manual
  reinstall. The existing atomic replacement/backup logic only runs after
  signature and archive checks have passed.
- If a key is compromised, pause release publishing and set the node's local
  `remote_upgrade_enabled = false` (or
  `AETHER_TUNNEL_REMOTE_UPGRADE_ENABLED=false`) and restart the service. This
  stops heartbeat-triggered upgrades; operators must also suspend manual
  self-upgrades. Verify a replacement binary and manifest with independently
  authenticated surviving/new public keys before manual installation. Install
  a binary that omits the compromised key and confirm its version before
  reenabling remote upgrades. Offline nodes need the same intervention when
  they return.

There is no remote key-fetch fallback, unsigned recovery mode, automatic bridge
selection, or immediate revocation of already installed trust sets. Windows
self-upgrade retains its existing manual-reinstall boundary.

## Offline verification and fixtures

With reviewed public inputs loaded, check a downloaded manifest before comparing
the requested archive's SHA-256 against its authenticated checksum entry:

```sh
.github/workflows/scripts/verify-tunnel-release.sh SHA256SUMS.txt SHA256SUMS.txt.sig
```

The publication/offline helper additionally pins `KEY_ID` to the expected signer;
set it to the reviewed key for a retained bridge. Do not copy that expectation
from an unverified envelope. Independently verify the archive digest before
extracting/installing; the helper authenticates the manifest only.

Run `bash tests/release_supply_chain_test.sh` for real OpenSSL signatures,
shared-parser unit tests, build-input conflicts, and binaries compiled with
legacy, overlap, and retired sets. Fixtures prove runtime environment changes
cannot alter embedded trust, and reject old signatures after retirement,
unknown/mislabeled keys, tampering, and malformed/duplicate/unknown inputs.
Test private keys are generated in a private temporary directory and removed
when the test exits. No production key, release upload, or production setting
is needed. Local fixture success does not replace per-platform release builds
and approved deployment evidence.
