# Issues 247 and 254: API Error Contract Decision

Updated 2026-09-12. Scope is the fork `JinPengGeng/aeris-token`; the upstream
repository is read-only.

## Verification

Issue 247 correctly identified that local wallet denial returned HTTP `429`
with a custom `balance_exceeded` payload and Chinese text. The existing
notification registry and dispatch infrastructure are present, so the old
claim that notification configuration is entirely disconnected is not an
accurate implementation target. Notification trigger coverage remains a
separate follow-up.

Issue 254 is a mixed report. Current source and tests show that OpenAI/Claude
formatters are already reused on execution paths and public model lookup
already emits/tests `model_not_found`. The remaining user-facing gaps are the
missing chat/images quickstart and the local OpenAI validation errors that used
`{ "detail": "..." }` with Chinese image messages.

## Decision

1. Keep wallet denial at HTTP `429`, matching OpenAI's quota-compatible
   response behavior. Distinguish it from transient rate limiting with
   `error.code=insufficient_quota`, English message `Insufficient quota`, and
   no `Retry-After` header. This avoids turning a permanent account state into
   an exponential-retry loop without introducing a breaking `402` contract.
2. Route OpenAI-family local validation errors through the existing shared
   formatter. The status mapping remains unchanged; Claude and Gemini
   envelopes remain format-specific.
3. Add executable user documentation for chat completions and images, plus a
   public error matrix that states retry and fail-closed conversion behavior.

## Validation

The implementation adds focused unit tests for the OpenAI envelope and wallet
quota payload. Existing image parsing and conversion tests remain unchanged;
the full gateway test suite and CI are required before merging.

## Residual work

The mixed Issue 247 notification and refund observations require separate
trigger/idempotency work and should not be closed by this contract change.
Likewise, documentation coverage for provider-specific examples may grow
independently without changing the stable error mapping above.
