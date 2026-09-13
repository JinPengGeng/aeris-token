# Public API Compatibility Fixtures

`fixtures/public-api-compatibility.json` is the reviewable contract matrix for
the residual part of Issues #247/#254. The formatter test checks each row's
envelope/type/code and the consistency of its status and retry metadata. That
test does not send requests to the listed endpoints, so a passing formatter
test alone does not establish the route's HTTP status or response headers.

The matrix covers OpenAI Chat Completions, Responses, Embeddings and Images,
plus Claude Messages. Issue #343 adds quota, permission and provider rate-limit
rows for all four text/embedding endpoints. Router tests exercise real local
wallet/key denials (including streaming requests); finalize tests exercise both
provider formats, HTTP-200 error bodies and preconverted error bodies. All
check the response trace header and actual Retry-After behavior.

The current fixture-driven HTTP coverage is:

| Rows | Actual route coverage |
| --- | --- |
| Quota and permission denials for Chat, Responses, Embeddings and Claude Messages | Authenticated Router tests read these rows and assert the actual error response, trace header and absence of `Retry-After`; quota requests also cover streaming before commitment. |
| `openai-images-invalid-request`, `openai-images-edits-invalid-request` | Authenticated Router tests submit the exact fixture payloads, assert `400`, the OpenAI envelope/type/code and `x-trace-id`, and verify no `Retry-After` or upstream execution. |
| Remaining rows | Formatter and metadata checks; endpoint/status/header coverage must not be inferred from the row alone. |

The Images generation-count overflow regression separately exercises its real
Router response with the same status/type/trace/retry-header checks. It is not
an additional fixture row. Images model-not-found and Chat invalid-request/
model-not-found rows still need route-level acceptance. In particular, missing
model fixtures describe the target contract: `GET /v1/models/:id` returning
`404/model_not_found` does not prove that inference POST requests do the same.
See the [current model-lookup boundary](error-contract.md#conversion-and-model-errors).

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
