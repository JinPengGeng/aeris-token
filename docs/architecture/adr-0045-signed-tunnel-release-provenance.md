# ADR-0045: Signed provenance for tunnel release upgrades

- Status: accepted for implementation
- Date: 2026-09-12
- Scope: `aether-tunnel` release assets and self-upgrade verification
- Related: #205, #315

## Decision

Tunnel release upgrades will use an offline-verifiable Ed25519 signature over
the published `SHA256SUMS.txt` manifest. The updater will verify the detached
signature before it parses or trusts any checksum, then verify the downloaded
archive against the signed manifest. A checksum without a valid signature is
not an acceptable trust anchor.

The release bundle will contain:

```text
SHA256SUMS.txt
SHA256SUMS.txt.sig
release-provenance.json
```

`SHA256SUMS.txt.sig` is a small versioned envelope containing a key identifier
and a base64 Ed25519 signature over the exact bytes of `SHA256SUMS.txt`.
`release-provenance.json` is informational and records the source commit,
workflow identity, release tag, and the manifest digest; the updater does not
use its network location as a trust root. The existing GitHub artifact
attestation remains an additional publisher/build audit signal, not a runtime
dependency.

The tunnel binary embeds a trust set of public keys keyed by the same stable
identifier. Verification succeeds only when the envelope is well formed, the
key identifier is known, the signature is valid, the manifest contains one
valid entry for the requested archive, and the archive digest matches that
entry. Missing, malformed, unknown-key, invalid-signature, duplicate-entry,
and digest-mismatch cases fail closed before extraction or replacement.

## Threat model and boundaries

This protects against a compromised release CDN, mirror, DNS path, or transport
that can replace an archive and its unsigned checksum file. It does not claim
to protect against compromise of the signing key, a malicious trusted release
workflow, or a tunnel binary that was already replaced. Release signing must
therefore run only in a protected environment after the release tag and
artifacts are reviewed.

The updater remains subject to its existing bounded-download, archive traversal,
atomic replacement, rollback, and version monotonicity checks. Signature
verification is an earlier gate; it does not weaken those checks or permit
automatic downgrade.

## Key custody and rotation

- The signing private key is held outside the repository in a protected release
  environment. It is never committed, printed, uploaded as an artifact, or
  included in a release archive.
- The release workflow signs the final `SHA256SUMS.txt` bytes and fails if the
  manifest changes afterward. The workflow records the key identifier and
  manifest digest in `release-provenance.json`.
- The trust set ships with the tunnel binary. A rotation adds the new public
  key before the release workflow starts using it; old keys remain accepted for
  the documented overlap window so an already-installed client can upgrade.
- Removing a retired key is a deliberate tunnel release and is never performed
  by changing a remote manifest. Emergency revocation requires shipping a new
  tunnel binary or disabling remote upgrades through the existing local
  operator control.

## Implementation contract

1. Add a small verifier with strict envelope and manifest parsing. Keep error
   messages stable and free of tokens, URLs containing credentials, or private
   key material.
2. Add the signature and provenance files to release and nightly tunnel
   artifacts. Sign after archive creation and checksum generation; verify the
   generated files in CI before publishing.
3. Verify the manifest signature in both manual and server-pushed upgrade paths
   before `extract_binary` is called.
4. Add offline fixtures for every rejection branch and a successful verification
   using a test-only key. Fixtures must never contain production key material.
5. Document the public-key fingerprint, rotation procedure, and recovery path
   for operators. A failed verification leaves the current binary untouched.

## Alternatives considered

### Sigstore-only verification

Sigstore keyless signing gives strong workflow identity and provenance, but a
small self-updater would need a pinned Fulcio/Rekor/TUF verification stack and
availability policy. That is a larger runtime dependency and makes offline
recovery ambiguous. It remains useful as a supplementary release attestation.

### Minisign-only verification

Minisign is a reasonable wire format, but introducing a command-line verifier
or an unreviewed parser into the tunnel would make the client trust an external
binary or add another format dependency. The protocol contract above can later
adopt a reviewed minisign-compatible envelope without changing the trust and
rotation decisions.

### HTTPS or SHA256SUMS alone

Transport security and a same-origin checksum do not authenticate the release
publisher. They remain useful transport/integrity layers but are explicitly
insufficient for this issue.

## Rollback and failure handling

Verification failure is a hard stop: retain the current executable, remove only
the bounded temporary download, and report the key identifier/error class. The
existing atomic replacement and backup rollback path applies only after a
verified archive has been extracted. Operators can recover by reinstalling a
known-good release manually; no automatic fallback may fetch an unsigned asset.
