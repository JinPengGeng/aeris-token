# Issue #220: advisory evidence artifacts

## Decision

The dependency audit workflow previously exposed advisory output only in job
logs. That is insufficient for the reviewed RSA exception in #303: a maintainer
must be able to retrieve the advisory IDs, scanner result and exception record
after a scheduled or protected-branch run. This slice adds short-lived,
read-only workflow artifacts for every Cargo and npm audit matrix entry.

Each artifact contains the scanner JSON output, stderr, commit/timestamp
metadata and, for the workspace Cargo audit, the exact
`.github/security/cargo-audit-exception.json` used by the gate. Artifact upload
runs with `if: always()` so failed audits remain diagnosable. The audit command
exit status is preserved; artifacts do not turn a failed advisory check green.
Artifacts are retained for 30 days, matching the exception review cadence.

## Scope and non-goals

- The existing fail-closed Cargo and npm checks are unchanged.
- The RSA exception remains exact and time-bounded; no advisory is globally
  ignored by this change.
- No credentials, package contents or generated secrets are uploaded.
- Hosted artifact availability is still verified by the required
  `Dependency Audit / check`; a local run is not production evidence.

## Verification

The workflow YAML and shell blocks are reviewed for pinned actions, explicit
exit-status preservation and complete coverage of the six tracked npm lockfiles
plus both Cargo lockfiles. After merge, the first scheduled or protected-branch
run should be linked from #220 and #303 with the artifact names and retained
expiry date.

The npm command explicitly requests `--json`; naming its redirected output
`audit.json` alone does not make npm's default human-readable report JSON.
`dependency-audit-artifacts.test.mjs` executes the actual workflow shell block
with a scanner fixture, parses the report and metadata, and verifies exit
statuses 0, 1 and 42 are preserved. This local contract does not replace a
hosted run or an audit against the live advisory database.
