# Automation AI Attestation Producer v3

This producer is a default-off, trusted-main workflow that creates the two check runs consumed by the Policy gate:

- `Automation Review Attestation / reviewer`
- `Automation Review Attestation / security`

It does not approve, merge, modify pull requests, or execute pull-request code.

## Trust boundary

`prepare`, `analyze`, and `finalize` all check out the repository default branch with credentials disabled. Pull-request title, body, and patches are API data. They are included in a canonical JSON input and are never evaluated as workflow, shell, tool, or model instructions.

Only `analyze` receives `AERIS_AI_API_KEY`; it receives no GitHub token or job permissions. Only `finalize` receives `checks: write`, through the workflow `GITHUB_TOKEN`. Persisted checks must therefore be owned by the GitHub Actions App (`15368`, `github-actions`). Reviewer and security remain distinct through their check names, roles, prompts, profiles, model evidence, and receipt hashes.

## Exact generation

Each successful receipt binds:

- repository ID/name and pull-request number
- exact head, base, and trusted-main Policy SHAs
- the initial or reopened pull-request lifecycle epoch
- canonical input, prompt, profile, result, and exact Git manifest/raw-diff hashes/evidence
- requested model and provider-reported response/model identity; provider model must equal the requested ID by default
- orchestration run group, job attempt audit ID, and completion time
- `verdict: pass` and `finding_count: 0`

Every finding, including low severity, prevents a success receipt. Missing patches, stale inputs, failed model calls, sensitive model output, invalid artifacts, timeouts, cancellation, and failed live revalidation produce no successful attestation.

`prepare` verifies the checkout origin, fetches the exact same-repository base commit and pull-ref head without tags or submodules, and runs bounded Git diff commands with external diff and textconv disabled. Binary content, submodules, invalid UTF-8, excessive files or bytes, ref races, remote drift, and API-to-Git path/blob disagreements fail closed. Git diff data is read as untrusted input; pull-request code is never checked out or executed.

## Check lifecycle

The workflow uses `prepare -> analyze -> always-finalize`. It does not create an `in_progress` check before inference. Once `prepare` has established a trusted candidate generation, `finalize` creates a new terminal check atomically and verifies the persisted App, head, external ID, conclusion, and output. If no trustworthy candidate can be established, the workflow fails closed without publishing a check; it never substitutes event payload data for a live generation. A finalizer publication error fails the workflow after attempting both roles.

Provider model aliases are not inferred. Supporting an alias requires a trusted-main profile to declare an explicit requested-to-reported model mapping and bind that mapping into the profile and receipt hashes before activation.

## Integration interface

Policy/Merger integration should import `review-attestation-contract.mjs`, select the highest-ID check for each exact check name and head SHA, require GitHub Actions App ownership, parse the canonical summary with `parseReviewAttestationSummary`, compare the embedded generation to the live Policy generation, require the reviewer and security receipts to have the same `run_id`, and require `check.external_id === reviewAttestationExternalId(receipt)` plus a completed `success` conclusion. The shared run ID prevents a partially published run from being combined with an older role success.

Activation additionally requires trusted configuration work outside this producer patch: enable the security role, validate the dedicated `AERIS_AI_ATTESTATION_ENABLED` switch and approved model/profile policy, and teach the Policy receipt contract to retain the attestation evidence hash. Until that integration is merged, leave the repository variable unset or false.
