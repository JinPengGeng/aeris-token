# Public API Compatibility Fixtures

`fixtures/public-api-compatibility.json` is the reviewable contract matrix for
the residual part of Issues #247/#254. It is loaded by the gateway test suite,
so changes to a status, envelope, error code, or retry header policy fail CI
until both the fixture and implementation are reviewed together.

The matrix deliberately covers the two endpoint families left out of the
baseline API contract PR #290: OpenAI Chat Completions and Images. Claude
Messages rows provide a cross-format guard so the OpenAI envelope does not
leak into the Anthropic surface.

Each row records:

- the public endpoint and client format;
- a minimal request payload;
- HTTP status and error envelope/type/code;
- whether `Retry-After` is expected and whether a client may retry.

`retryable` is independent from the header: transient overloads may be retried
with bounded backoff even when the gateway has no provider-supplied
`Retry-After`; when a header is present it is always a positive number of
seconds. `insufficient_quota` is the explicit non-retryable exception.

`insufficient_quota` is a permanent wallet state. It remains HTTP `429` for
OpenAI client compatibility but intentionally omits `Retry-After` and is marked
non-retryable. The fixture does not claim that balance notifications or refund
transitions from Issue #247 are complete; those remain separate acceptance
work.

Related contracts:

- [Chat Completions](chat-completions.md)
- [Images](images.md)
- [Public API Error Contract](error-contract.md)
- [Issue 247/254 decision](../issue-triage/issue-247-254-api-contract-decision.md)
