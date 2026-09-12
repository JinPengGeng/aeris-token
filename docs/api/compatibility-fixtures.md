# Public API Compatibility Fixtures

`fixtures/public-api-compatibility.json` is the reviewable contract matrix for
the residual part of Issues #247/#254. It is loaded by the gateway test suite,
so changes to a status, envelope, error code, or retry header policy fail CI
until both the fixture and implementation are reviewed together.

The matrix covers OpenAI Chat Completions, Responses, Embeddings and Images,
plus Claude Messages. Issue #343 adds quota, permission and provider rate-limit
rows for all four text/embedding endpoints. Router tests exercise real local
wallet/key denials (including streaming requests); finalize tests exercise both
provider formats, HTTP-200 error bodies and preconverted error bodies. All
check the response trace header and actual Retry-After behavior.

Each row records:

- the public endpoint and client format;
- a minimal request payload;
- HTTP status and error envelope/type/code;
- whether `Retry-After` is expected and whether a client may retry.

`retryable` is independent from the header: transient overloads may be retried
with bounded backoff even when the gateway has no provider-supplied
`Retry-After`; when a header is present it is always a positive number of
seconds. `insufficient_quota` is the explicit non-retryable exception.

`insufficient_quota` is an account state requiring restored credit. It remains
HTTP `429` for OpenAI client compatibility, with `credit_balance_exhausted` as
the error code. Claude uses `402/billing_error` with `balance_exceeded`. Both
intentionally omit `Retry-After` and are marked non-retryable. Retry guidance is
client advice, not a claim that every SDK disables automatic retries on 429.
The fixture does not claim that balance notifications or refund
transitions from Issue #247 are complete; those remain separate acceptance
work.

Related contracts:

- [Chat Completions](chat-completions.md)
- [Images](images.md)
- [Public API Error Contract](error-contract.md)
- [Issue 247/254 decision](../issue-triage/issue-247-254-api-contract-decision.md)
- [Issue 343 quota decision](../issue-triage/issue-343-quota-contract.md)
