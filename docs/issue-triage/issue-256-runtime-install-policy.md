# Issue #256 runtime tunnel install policy

Status: Ready / P2 (residual acceptance gate)

Date: 2026-09-12

## Decision

Keep the runtime-generated tunnel installer URLs and the tunnel installer
default release repository pointed at `fawney19/Aether` until this fork has a
published `tunnel-v*` release. Switching the defaults before that release
exists would make the generated install command return a 404 and would be a
regression for users of the current gateway.

The repository and installer remain overrideable through
`AETHER_TUNNEL_RELEASE_REPO`; this is the supported path for operators who
publish tunnel artifacts in a private or downstream repository.

## Evidence (2026-09-12)

- `gh release list --repo JinPengGeng/aeris-token` contains
  `aeris-token-v0.1.0` and `aeris-token-nightly-20260911`, but no `tunnel-v*`
  release.
- `.github/workflows/build-tunnel.yml` publishes tunnel artifacts only when a
  `tunnel-v*` tag is pushed, and the release job uses `${{ github.repository }}`.
  The workflow is therefore ready to publish fork-owned artifacts, but has not
  produced one yet.
- `apps/aether-gateway/src/handlers/public/support/install.rs` emits the
  upstream raw installer URLs. `apps/aether-tunnel/install.sh` and
  `install.ps1` use the same upstream repository as their safe default and
  validate any override before downloading.

## Migration gate

When the fork publishes its first non-draft `tunnel-v<semver>` release, one
follow-up change must update all of these together:

1. The two runtime URL constants in `install.rs`.
2. The shell and PowerShell installer default repository.
3. The tunnel README download and bootstrap links (the release workflow will
   maintain the generated download table after the first release).
4. The installer security test fixtures and this decision record.

The follow-up PR must verify the release assets, `SHA256SUMS.txt`, provenance
attestation, and a successful install-session response for both shell and
PowerShell URLs before changing the Issue status to Done. Until then, #256's
runtime URL item remains explicitly deferred rather than silently claiming
completion.

## Re-check commands

```sh
gh release list --repo JinPengGeng/aeris-token --limit 100
rg -n 'tunnel-v|fawney19/Aether|AETHER_TUNNEL_RELEASE_REPO' \
  apps/aether-gateway/src/handlers/public/support/install.rs \
  apps/aether-tunnel/install.sh apps/aether-tunnel/install.ps1
```

