# Issue 412: Missing public API credentials

Updated 2026-09-14. Child of [Issue 254](https://github.com/JinPengGeng/aeris-token/issues/254),
scoped to `JinPengGeng/aeris-token`.

## Cause and change

The isolated Gateway exercise in #409 found that a Chat request with no
credential returned `503/server_error` and `missing_auth_context`, while an
unknown Bearer API key returned `401/authentication_error`. The public auth
resolver returned an empty context without a rejection when its existing
credential extractor found nothing. The request then reached execution
planning, which correctly treats a missing internal context as a server error.

The public auth resolver now records the existing `InvalidApiKey` rejection
when an `ai_public` decision has a nonempty auth endpoint signature, no resolved
identity, no previous rejection, and neither a selected credential nor verified
trusted identity headers. The existing rejection response produces HTTP `401`,
the route's current error envelope, `x-trace-id` and no `Retry-After`. It does not
invoke upstream execution. Empty or malformed credential carriers that the
existing extractor does not accept follow the same rejection path.

Credential selection and precedence are unchanged. Existing Bearer/API-key
headers, query keys, deferred cookies/Google Bearer handling, and verified
tunnel affinity identity keep their current resolution paths. A missing or
empty internal auth endpoint signature is not reclassified as external
credential absence. Already-resolved identity is also preserved if original
headers are no longer present.

## State and failure priority

| Condition | Result |
| --- | --- |
| Local or distributed request admission rejects the request | Existing overload response, before authentication. Redis admission failure remains `503/server_error`. |
| No accepted credential or verified identity at the public authentication boundary | Existing `401` authentication rejection; no upstream or retry header. |
| Credential exists but is an unknown API key | Existing `401` authentication rejection. |
| Credential resolves successfully | Existing model/policy/selection and execution flow. |
| Credential resolution is deferred or internal execution context is unavailable | Existing failure handling; no global `missing_auth_context` remapping. |
| Forged trusted headers from untrusted ingress | Headers are stripped; absent real credentials now produce `401`. Verified affinity requests retain their existing identity and authorization behavior. |

The implementation does not change admission, the planner, or public response
construction. Existing fallback tests now explicitly supply a credential while
omitting the authentication reader: they continue to test internal unresolved
context and the original `503` response. They no longer use external anonymous
requests to represent that internal failure.

Payload and file-existence fixtures now authenticate before exercising their
original assertions. Embedding/Rerank validation reuses their existing valid
API-key and execution-runtime fixtures and still expects `400`. Gemini Files
download uses a valid identity plus an empty in-memory file-mapping repository
and still expects `404`; supplying identity without a mapping reader correctly
returned `503` during the first focused run, so that incomplete fixture was
replaced with a real empty lookup. No production file lookup behavior changed.
Video and Files upload runtime-miss fixtures explicitly retain absent auth
readers while supplying credentials, preserving their existing `503` coverage.

## Validation

The new authenticated Router fixtures use an in-memory auth repository and a
counted execution-runtime server wired into the Gateway. They cover missing,
empty, malformed, duplicate and invalid authorization, empty API/query keys,
both Chat sync and stream requests, adjacent public formats using the common
boundary, and all five existing OpenAI API-key carriers. The valid-carrier
fixture deliberately has no provider candidate, so its expected `503` is a
selection boundary rather than a claimed successful provider response.

A separate real Router test starts an isolated managed Redis, creates the
distributed request semaphore, stops that Redis, then sends a credential-free
Chat request. It verifies that distributed admission still wins with `503` and
no upstream call. Like existing managed Redis tests, it reports a skip when
`redis-server` is unavailable; the recorded local validation must distinguish
that condition from an executed outage test.

Focused resolver tests preserve an already-resolved identity and leave missing
internal route metadata or deferred credential handling unchanged. Existing
credential extraction, signed affinity, invalid-key and fallback regressions
are included in validation.

Validation was executed with Rust 1.95 and `RUST_MIN_STACK=16777216`, matching
Gateway CI, from base `2005db2994bf9d39bc52bdd5aaa5c73b75b950da` plus this diff.
The local shared target cache was exclusively assigned to this worktree.

```sh
rtk proxy env PATH="/Users/fengying/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin:$PATH" \
  CARGO_TARGET_DIR=/private/tmp/aeris-api-contracts/target RUST_MIN_STACK=16777216 \
  cargo test -p aether-gateway --lib -- \
  tests::control::proxy::missing_credentials control::auth::resolution::tests \
  tests::ai_execute::fallback tests::ai_execute::control_execute \
  tests::control::proxy::embeddings tests::control::proxy::rerank \
  tests::files tests::video::routing \
  gateway_strips_forged_trusted_auth_headers_from_untrusted_ingress --test-threads=4

rtk proxy env PATH="/Users/fengying/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin:$PATH" \
  CARGO_TARGET_DIR=/private/tmp/aeris-api-contracts/target RUST_MIN_STACK=16777216 \
  cargo test -p aether-gateway --lib -- --test-threads=4 --quiet

rtk proxy env PATH="/Users/fengying/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin:$PATH" \
  CARGO_TARGET_DIR=/private/tmp/aeris-api-contracts/target RUST_MIN_STACK=16777216 \
  cargo clippy -p aether-gateway --all-targets -- -D warnings

rtk proxy env PATH="/Users/fengying/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin:$PATH" \
  cargo fmt --all --check
rtk proxy git diff --check
```

The final focused set passed **83 tests, 0 failed, 0 ignored** in 1.67 seconds.
It includes real positive Embedding/Rerank/Files execution, signed affinity and
the preserved internal fallback cases. The first 46-test auth-focused run also
passed with `--nocapture`; Redis 8.10.1 was installed and the managed Redis
outage test executed rather than taking its missing-binary skip branch.

The full Gateway library run reported **5477 passed, 1 failed, 19 ignored** in
38.04 seconds. All 34 old credential-free fixture failures from the initial
draft are resolved. The sole failure is the independently reproduced existing
`execution_runtime::windsurf::tests::windsurf_language_server_binary_must_be_trusted_and_already_executable`:
the fixture expects its executable under the current working directory to be
trusted, but this worktree is under `/private/tmp`, whose verified mode is
`drwxrwxrwt`. The unchanged validator rejects group/other-writable ancestors.
The failure is at `windsurf.rs:4583`, with
`Windsurf language server binary path is missing or unsafe`. No Windsurf source
or test was changed, ignored, or bypassed; this is **not** a fully green local
library run. The main thread then ran the same final Gateway test binary from
an isolated directory under `/Users/fengying/workspace`, after verifying that
every ancestor was trusted and not group/other writable. The exact Windsurf
test passed **1 / 0 failed / 0 ignored** in 0.01 seconds. This resolves the
local path prerequisite without weakening executable validation; the original
full-run result above remains recorded rather than being relabeled as green.

Gateway **all-targets Clippy passed** with `-D warnings` in 3 minutes 49 seconds.
`cargo fmt --all --check` and `git diff --check` passed. Independent review,
integration onto current `main`, and all four protected GitHub contexts remain
required before merge.

The main thread subsequently merged main `44bd242e15572e17cb808d2b9db213a443ab16cd`
(Images Router contracts and Redis reconnection fix) without conflicts. The
public error guide now describes the 401 boundary and Redis-admission priority.
Final-head regression checks and independent review are recorded in the PR;
the earlier local counts above refer to their stated baseline.

## Compatibility, remaining scope and rollback

Clients that retried anonymous Chat requests on `503` now receive the existing
authentication failure contract and should provide credentials. The message
remains the current localized invalid-key message; this change does not add
language negotiation or new public error fields.

Issue #254 remains open for unknown-model inference classification and other
documented contract gaps. Deferred cookie/Google Bearer resolution is not
redesigned, and this test slice does not establish every provider or CLI login
workflow. There is no data migration or credential change. Revert this commit
to restore the previous missing-credential response behavior.
