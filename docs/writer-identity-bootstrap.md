# Writer App identity bootstrap

This is a one-time recovery procedure for discovering the immutable GitHub App node ID and owner database ID that the normal Writer attestation must treat as trusted configuration. It does not weaken the normal attestation when either variable is absent.

## Safety boundary

`.github/workflows/writer-identity-bootstrap.yml` is manual, has no inputs, and runs only from `main` in the `writer` Environment when the actor and repository are exactly `JinPengGeng` and `JinPengGeng/aeris-token`. Its `GITHUB_TOKEN` has only `contents: read`.

The workflow is default-off. It requires all of the following repository variables before its job can start:

- `AERIS_WRITER_IDENTITY_BOOTSTRAP_ENABLED=true`
- `AERIS_WRITER_GOVERNANCE_CANARY_ENABLED=true`
- `AERIS_AGENTS_ENABLED=false`
- `AERIS_CANDIDATE_AGENTS_ENABLED=false`
- `AERIS_WRITER_ENABLED=false`
- `AERIS_UPSTREAM_SYNC_ENABLED=false`
- `AERIS_AUTONOMOUS_MERGE_ENABLED=false`

The canary flag must already be enabled so the subsequent governance-proof job cannot silently skip. Bootstrap never enables it or any production switch.

The `writer` Environment must temporarily contain `AERIS_IDENTITY_BOOTSTRAP_TOKEN`. Use a short-lived fine-grained owner token limited to this repository, with only repository **Actions: read and write** and **Variables: read and write** permissions (plus GitHub's implicit metadata read). The token exists solely because the read-only `GITHUB_TOKEN` and the minimally privileged Writer App cannot update repository variables. Never grant the Writer App additional permissions for bootstrap.

GitHub does not expose a complete API inventory of every repository selected on a fine-grained personal access token. The workflow therefore cannot independently prove that this control token has no access to another repository. Repository-only selection, the two named permissions, and the short expiry are mandatory operator gates. The CLI does prove `/user` is exactly `JinPengGeng` (`databaseId=36217715`, `type=User`), rejects any non-empty or missing `X-OAuth-Scopes` header (and therefore classic/OAuth-scoped tokens), and proves the token can read repository ID `1316750512`. When GitHub returns `GitHub-Authentication-Token-Expiration`, the CLI requires a future expiry no more than 24 hours away and records it in the non-sensitive summary. GitHub may omit that header; absence is reported as unverified and is not treated as proof of a short expiry.

## Verified operation

The CLI creates a short-lived App JWT from the existing `AERIS_WRITER_APP_PRIVATE_KEY` Environment secret. It does not print or persist the private key, JWT, control token, authorization headers, or raw API responses.

Before any governance write, it reads `/app` and `/app/installations/155342531` twice in sequence. Both normalized snapshots must be identical. Using the App JWT, it then mints one installation token whose requested permission is only `contents:read`; it does not restrict the token to a named repository, because doing so would hide an over-broad installation selection. The returned permission set must contain only `contents:read` and optional implicit `metadata:read`, and its expiry must be in the future and no more than 65 minutes from the proof clock. That token performs two complete, bounded, `per_page=100` paginated reads of `/installation/repositories`. Each full inventory must contain exactly one repository, and both normalized inventories must be identical. The combined proof must establish:

- App ID `4667256`, slug `aeris-token-writer`, and owner `JinPengGeng`;
- installation ID `155342531`, the same owner identity, selected-repository mode, and no suspension;
- actual installation-token inventory of exactly `JinPengGeng/aeris-token`, repository ID `1316750512`, owned by user ID `36217715`, with default branch `main`;
- exactly `administration:read`, `checks:write`, `contents:write`, `pull_requests:write`, and implicit `metadata:read`;
- no subscribed App or installation events;
- execution in repository ID `1316750512` on `main` by the fixed owner actor.

The CLI also uses the control token to canonical-double-read all five production switches plus the bootstrap and governance-canary flags. Every snapshot revalidates the control-token user, exact target repository, all seven variable values, and `refs/heads/main`. The live branch head must remain exactly the workflow's immutable `GITHUB_SHA`. This guard runs before and after every variable write. Before bootstrap is disabled it additionally reads both identity variables and requires their exact discovered values; the final double-read repeats that complete binding after the flag is `false`. Any missing credential, unsafe flag, identity mismatch, extra permission/event, incomplete inventory, API failure, main movement, variable drift, or between-read drift fails closed. Once drift is observed, no further write is attempted.

After proof, the CLI performs only this ordered control sequence. Each numbered action is surrounded by the live double-read guard, and both identity-variable writes are read back exactly:

1. Create or update `AERIS_WRITER_APP_NODE_ID`.
2. Create or update `AERIS_WRITER_APP_OWNER_DATABASE_ID`.
3. Set `AERIS_WRITER_IDENTITY_BOOTSTRAP_ENABLED=false`.
4. Complete the final guarded readback of both identity variables and the disabled bootstrap state.

After that job succeeds, the same bootstrap workflow run calls the local reusable `writer-readonly-attestation.yml` and then `writer-governance-canary.yml`. GitHub resolves local reusable workflows from the caller's immutable workflow revision, so bootstrap never uses the REST workflow-dispatch API or a moving branch ref. The called jobs do not receive the bootstrap control token. The normal read-only attestation repeats the exact one-repository Writer-token proof; the governance canary independently proves the full governance fence.

The summary contains only the two discovered IDs, normalized non-sensitive identity/control facts, and SHA-256 digests. It never contains either token, the JWT, authorization headers, the private key, or raw responses.

## Operator procedure

1. Create the short-lived fine-grained token described above. In the GitHub UI, explicitly select only `JinPengGeng/aeris-token`, grant only Actions and Variables read/write, and set an expiry no later than the planned bootstrap window. Preserve a screenshot or approval record of those three operator-gated settings because the API cannot fully enumerate them.
2. Add it as the `writer` Environment secret `AERIS_IDENTITY_BOOTSTRAP_TOKEN`.
3. Confirm every production switch listed above is exactly `false`.
4. Set `AERIS_WRITER_GOVERNANCE_CANARY_ENABLED=true`, then set `AERIS_WRITER_IDENTITY_BOOTSTRAP_ENABLED=true`.
5. Manually dispatch **Writer identity bootstrap** from `main` as `JinPengGeng`.
6. Confirm the bootstrap run succeeded, the summary names repository ID `1316750512` and the trusted `main` SHA, the bootstrap flag now reads `false`, and its two local reusable proof jobs succeeded rather than skipped. If the control-token expiration header is reported unavailable, use the operator record from step 1 as the expiry evidence; do not infer an API-verified expiry.
7. Immediately delete the `AERIS_IDENTITY_BOOTSTRAP_TOKEN` Environment secret and revoke/delete the short-lived token at its issuer.
8. Set `AERIS_WRITER_GOVERNANCE_CANARY_ENABLED=false` and verify it by API/UI readback.
9. Preserve the bootstrap run URL, the two called-job links, and all non-sensitive step summaries as rollout evidence.

If the run fails after one identity variable was written or after disabling its flag, no later action is attempted. Do not broaden App permissions or leave the temporary credential installed. Delete/revoke the temporary token, inspect the non-sensitive failure reason, restore all seven switches to the required closed/bootstrap state, verify `main` intentionally points to the workflow commit, and start a new explicitly enabled bootstrap attempt. A partial identity-variable write is inert while all production switches remain `false`, but it must be reviewed and overwritten by the next successful guarded run.
