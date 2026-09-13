# Issue 406: Images public Router contract

Updated 2026-09-14. Child of [Issue 254](https://github.com/JinPengGeng/aeris-token/issues/254),
scoped to the fork `JinPengGeng/aeris-token`.

## Evidence and decision

Chat and Images documentation already exists, and OpenAI local errors already
use the shared formatter. PR #335 added the compatibility matrix; later quota
work added real HTTP coverage for the text/embedding quota and permission rows.
The remaining Images validation rows were still checked only by the formatter
test, which cannot observe routing, HTTP headers or actual status selection.

This change reuses `openai-images-invalid-request` and
`openai-images-edits-invalid-request` from the existing JSON matrix. A real
authenticated Router receives each row's exact request payload. The test
checks HTTP `400`, the OpenAI envelope/type/code, the English field message,
`x-trace-id`, no `Retry-After`, and the local public-validation execution path.
An execution-runtime override points to a counted local upstream server, which
must receive zero calls. The existing `n` overflow Router regression now uses
the same connected upstream guard and checks the error type and headers.

The only production behavior change is the local image-count error message:
`Image requests require n between 1 and N` (or the selected-model form).
The accepted count range and HTTP status do not change. Its prior exact unit
and Router assertions are updated. Clients should interpret status/type/code
rather than depending on translated message text.

README links now expose the existing Chat quickstart, Images examples and error
contract. API documentation separates actual route coverage from formatter
checks, and qualifies the language and model-lookup guarantees against current
source. No additional fixture schema, scheduling policy or provider mapping is
introduced.

## Validation

Use the repository's Rust 1.95 toolchain. These focused checks cover the new
route test, the existing count regressions and the existing formatter matrix:

```bash
export RUST_MIN_STACK=16777216
cargo test -p aether-gateway --lib gateway_image_invalid_request_fixtures_match_real_router_responses -- --nocapture
cargo test -p aether-gateway --lib gateway_rejects_image_request_above_gateway_limit_without_hitting_fallback_probe -- --nocapture
cargo test -p aether-gateway --lib image_validation_applies_the_global_count_limit_before_model_mapping -- --nocapture
cargo test -p aether-gateway --lib public_api_compatibility_fixture -- --nocapture
cargo clippy -p aether-gateway --lib --tests -- -D warnings
cargo fmt --all -- --check
git diff --check
```

On macOS with Rust 1.95.0, the four focused tests passed (4 passed, 0 failed,
0 ignored; 0.06 seconds excluding compilation). The new test covers both
existing Images fixture rows. Formatting, diff validation and 15 local
documentation links/anchors passed. Package Clippy for the library and tests
passed with `-D warnings` (4 minutes 24 seconds).

The first local invocation used the default test-thread stack and aborted with
a stack overflow. Re-running with `RUST_MIN_STACK=16777216`, matching the
existing gateway CI configuration in `.github/workflows/rust-ci.yml`, passed;
no production stack setting changed. Use the same test environment when
reproducing these Router regressions.

The root agent independently reviewed the implementation and test wiring,
including the counted upstream connected through the execution-runtime
override, and accepted the bounded change. It independently reran the new
Router test: one passed, zero failed/ignored, covering both fixture rows in
0.05 seconds. This separates implementation and review between agents; it
does not claim a second human maintainer approval. The final PR head still
requires all four protected GitHub contexts before merge.

## Remaining acceptance and rollback

Issue #254 remains open. `GET /v1/models/:id` has a model-not-found response,
but the inference fallback in `handlers/proxy/mod.rs` still maps an empty
candidate list to `503` unless capacity classification selects `429`. A model
that does not exist must be distinguished from an existing model with no
currently available provider before changing this behavior. Existing
Chat/Images model-not-found fixture rows express a target contract; this slice
does not claim they pass through real inference routes.

Other public authentication, policy and execution messages may still be
Chinese, with no `Accept-Language` contract. Provider-specific success examples
and the remaining fixture routes also retain their separate acceptance. The
notification/refund work under #247 is outside this child issue.

There is no data migration. Revert this change to restore the earlier local
image-count wording and documentation; routing behavior is unchanged.
