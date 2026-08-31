# GitHub App Bootstrap

This directory provisions four independent private Apps: Writer, Policy, Merger, and Sync. Reviewer and Security remain on the GitHub Actions identity. The tool never enables an App-backed workflow.

## Commands

`node tools/github-app-bootstrap/bootstrap.mjs --dry-run` validates local configuration only. It performs no network or GitHub operation.

`node tools/github-app-bootstrap/bootstrap.mjs --serve --apply` is the only mutation mode. Before its first mutation it requires the existing `release` Environment to contain a required-reviewer rule with at least one human/team reviewer, then records that baseline. It sets all four activation variables to `false` with readback and makes the four role Environments reviewer-free, wait-free, and limited to the `main` deployment branch. It never mutates `release` and rejects any baseline drift.

Apply mode prints a one-time loopback initiation URL. Each role then follows one same-process sequence:

1. Create the private, webhook-free App from its manifest.
2. Atomically checkpoint `conversion_started` before calling GitHub's one-shot conversion endpoint. Validate the returned owner, App ID, slug, and RSA private key, then atomically checkpoint the non-secret App ID/slug with status `converted` before attempting secret or variable configuration.
3. Store the private key in only the matching Environment secret, store mapped non-secret variables, and read back every readable fact while the activation variable remains `false`.
4. Use the displayed owner-restricted installation URL and select only `JinPengGeng/aeris-token`.
5. Return to the loopback verification URL. The tool signs an App JWT from the in-memory PEM, verifies the live App ID, slug, owner, exact permissions, and empty event list, and requires exactly one selected installation with the same permissions/events. Its temporary installation token request explicitly names only the expected repository ID and the role's exact permissions. The returned scope is checked, the repository list must contain exactly the expected repository ID/name, and the token is revoked in `finally` on both success and failure. Revocation failure blocks verification.

The PEM, App JWT, installation token, manifest conversion code, and loopback capabilities are never written to a receipt or command argument. A conversion transport failure, invalid response, or process exit is always result-unknown because GitHub may already have created the App. The durable `conversion_started` state therefore permanently disables automatic conversion retry for that role. A later process opens a reconciliation form; the operator supplies the created App ID and downloaded PEM in a size-limited local POST. The PEM must sign a valid App JWT whose live App ID, fixed slug/name, owner, exact permissions, and empty event list all match before the tool checkpoints that identity and continues. If the App was never created or the App ID/PEM is unavailable, bootstrap remains blocked; an operator must investigate and deliberately resolve the orphaned conversion rather than create another App through this tool. A normal post-conversion restart fixes the expected App ID/slug and follows the same authenticated adoption check. Configuration failures remain directly recoverable while the original process still holds the PEM.

Every apply process first acquires `receipts/.bootstrap.lock` and holds it until the loopback server closes. A second process fails closed before GitHub readback or mutation; a lock left by an abnormal process exit must be removed only after confirming that no bootstrap process is running. Inside the server, one fail-fast transition lock covers each complete request state transition, including remote calls and receipt writes; overlapping requests receive `409` and cannot issue a retry capability or mutate coordinator state. Every transition writes `receipts/latest.json` through a same-directory temporary file and atomic rename. Receipt schema v5 is closed, secret-scanned, bound to the canonical digest of roles, permissions, manifests, Environment policy, reviewer identity, and rulesets, and records each private-key secret's non-secret `updated_at` version. It also records point-in-time App-authenticated PEM probe evidence and any role whose fail-closed disable readback failed. Verified values must equal the current expected names/slugs/ruleset digests rather than merely matching types. A successful receipt is emitted only after all four Apps have distinct IDs/slugs, all four App-authenticated global installation and Environment checks pass, all secret names/versions are stable, every switch reads back exactly `false`, and the ending `release` snapshot exactly matches the approved starting snapshot. Partial resume performs live readback of every previously verified role before any mutation and preserves App checkpoints and prior verified switch evidence even when persistent fail-closed disabling fails; `disable_failures` makes that condition explicit and blocks a verified receipt.

Schema v4 and older receipts are intentionally rejected and never overwritten. Review and archive any older receipt only after establishing its live GitHub state; do not delete it merely to bypass the v5 checks.

A local SHA-256 digest is not treated as proof of receipt authenticity. Every verified rerun performs bounded read-only GitHub readback of repository identity, the approved `release` snapshot, both rulesets, all role variables, the Writer public-key digest, secret metadata, role Environments, public App identity/permissions/events/version, user installation identity/permissions/events, and the single installed repository. Only exact live equivalence returns the existing receipt byte-for-byte without mutation. A structurally self-consistent forgery cannot bypass this readback: any forged claim that differs from live state fails, while a document that exactly describes live state is accepted on the authority of the fresh GitHub readback rather than its local digest. Replay after secret rotation/deletion, App metadata changes, installation revocation, configuration drift, pagination ambiguity, or network failure fails closed and never overwrites the prior receipt. GitHub does not expose an Environment secret value for readback, so the receipt's `pem_probe` proves only the PEM used during provisioning. A verified rerun does not claim that the current Environment secret is still that active key.

Local manifests are exact closed sets. OAuth callbacks, setup URLs, webhook URLs/secrets, OAuth-on-install flags, extra hook attributes, and every other undeclared field are rejected. The only redirect is added in memory for the one-time loopback manifest submission.

Validate both rulesets with `node tools/github-app-bootstrap/verify-ruleset.mjs`. The Writer branch ruleset applies only to `agent/**`, blocks deletion and non-fast-forward updates, and permits normal fast-forward Writer updates. The separate Merger tag ruleset applies only to `refs/tags/aeris-merger-attempt-*`, permits the first creation, then blocks update, deletion, and non-fast-forward mutation. Neither ruleset grants a bypass actor. Apply mode upserts and reads back both payloads; missing or drifted live rulesets block activation and verified receipts.

## Merger activation runbook

Merger activation is blocked unless `receipts/latest.json` is `verified`, `disable_failures` is empty, `AERIS_MERGER_ENABLED` reads exactly `false` during provisioning, and `rulesets.merger_generation_tags` contains `verified: true` plus a non-null digest. Before any later activation, rerun the same live ruleset readback and require the exact tag target, exact `refs/tags/aeris-merger-attempt-*` include pattern, exact `deletion`/`non_fast_forward`/`update` rules, `update_allows_fetch_and_merge: false`, active enforcement, and an empty bypass list. Missing, duplicate, inherited-only, disabled, or drifted rulesets are activation blockers.

Immediately before enabling Merger, run the activation probe inside the matching `merger` Environment, mapping only that Environment's variable and secret to the probe process:

```yaml
jobs:
  probe-merger-before-activation:
    runs-on: ubuntu-latest
    environment: merger
    steps:
      - uses: actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09
      - name: Prove Merger App key and installation scope
        env:
          AERIS_APP_ID: ${{ vars.AERIS_MERGER_APP_ID }}
          AERIS_APP_PRIVATE_KEY: ${{ secrets.AERIS_MERGER_PRIVATE_KEY }}
        run: node tools/github-app-bootstrap/probe.mjs --role merger
```

The probe signs an App JWT from the current Environment PEM, requires exactly one global `/app/installations` result with exact permissions and no events, mints an exact-permission installation token, requires its repository list to contain only `JinPengGeng/aeris-token`, and revokes the token in `finally`. Any probe error, timeout, pagination signal, revocation failure, identity drift, extra installation, extra repository, or scope mismatch blocks activation. The same probe with the matching role Environment is a mandatory hard gate before enabling Writer, Policy, or Sync; a receipt alone is never sufficient for activation.
