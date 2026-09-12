# Public API Error Contract

This document records the error contract at the public gateway boundary. It
describes the current fork implementation, not every provider's native error
format. `x-trace-id` is present on gateway responses for correlation.

The executable compatibility rows for Chat Completions, Responses, Embeddings,
Images, and Claude Messages live in [Public API Compatibility Fixtures](compatibility-fixtures.md).

## OpenAI-family envelope

OpenAI Chat, Responses, Embeddings, Rerank, and Images validation errors use:

```json
{
  "error": {
    "message": "Image API JSON request body is invalid",
    "type": "invalid_request_error"
  }
}
```

The `error.code` field is included when a stable code is available. The
gateway's generic mapping is:

| HTTP status | `error.type` | Retry guidance |
| --- | --- | --- |
| 400/405/422 | `invalid_request_error` | Fix the request; do not retry unchanged. |
| 401 | `authentication_error` | Replace credentials. |
| 403 | `permission_error` | Change policy, key, or model access. |
| 404 | `not_found_error` | Check the path/resource; a missing model uses `model_not_found`. |
| 413 | `context_length_exceeded` | Reduce request size. |
| 429 | `rate_limit_error` | Retry only when the response identifies a transient rate/quota window. |
| 429 | `insufficient_quota` | Restore account credit; do not retry unchanged. |
| 503/529 | `server_error` | Retry with bounded backoff when `Retry-After` is present. |

## Quota versus rate limit

Wallet denial is a permanent account state, not a temporary RPM window. For
OpenAI-family routes it remains HTTP `429` for compatibility with clients that
recognize OpenAI's quota response, but has this stable payload:

```json
{
  "error": {
    "message": "Insufficient quota",
    "type": "insufficient_quota",
    "code": "credit_balance_exhausted"
  }
}
```

Claude Messages wallet denial uses HTTP `402` and
`{"type":"error","error":{"type":"billing_error","code":"balance_exceeded","message":"Insufficient quota"}}`.
Neither format exposes the balance, user/key identifiers, or internal billing
details. Both retain `x-trace-id` for support correlation.

No `Retry-After` header is emitted for wallet denial or tenant permission
denial (`403/permission_error`). Provider rate limits remain `429/rate_limit_error`;
the gateway retains a provider-supplied wait time and does not invent one when
none is known. Daily usage, plan, and
RPM windows remain separate errors and include `Retry-After` plus their
existing `X-RateLimit-*` or `X-Daily-Usage-*` headers. Clients must not treat
`insufficient_quota` or `billing_error` as a backoff-only event. Some SDKs retry
all HTTP `429` responses automatically, even without `Retry-After`; applications
should inspect the error type/code and disable such retries for exhausted credit.

Streaming requests rejected before stream commitment use the same HTTP JSON
error. An error after commitment stays inside the protocol's terminal SSE event;
its quota type/code are preserved when converting between Chat, Responses and
Claude, while the already-sent HTTP status cannot change.

See [Issue 343 decision and migration](../issue-triage/issue-343-quota-contract.md)
for changes from the earlier `rate_limit_error/insufficient_quota` contract.

## Control-plane dependency failures

Failures while reading the control-plane data dependencies (Postgres/SQL,
Redis, or a bounded backend timeout) are projected as a retryable gateway
error. The response deliberately hides the backend error text:

```json
{
  "error": {
    "message": "gateway control unavailable",
    "type": "server_error",
    "code": "control_unavailable",
    "trace_id": "trace-...",
    "retryable": true,
    "failover_disposition": "retry_request"
  }
}
```

The current HTTP status is `502` and `Retry-After: 1` is emitted. Clients may
retry the request with bounded backoff and should use `x-trace-id`/`trace_id`
for support correlation. Invalid input, invalid configuration, and
unexpected stored values remain internal/operator errors and are not
classified as a transient dependency outage. This contract does not choose
Redis fail-open or fail-closed behavior; that remains a deployment policy
decision.

## Other public formats

Claude Messages uses the Anthropic envelope (`type=error` with a nested
`error` object), including `402/billing_error` for exhausted credit. Gemini
retains its existing `error.code/message/status` envelope and resource-exhaustion
semantics; this decision does not introduce a Gemini billing contract.
Provider error bodies are not copied blindly across the public boundary;
non-client provider failures are projected to a generic gateway error to avoid
credential or internal URL disclosure.

## Conversion and model errors

Cross-format conversion is fail-closed. A field with no audited lossless
mapping returns a structured conversion error instead of being silently
dropped. Unknown public models return HTTP `404` with
`error.code=model_not_found`. These outcomes are permanent request/configuration
failures and must not be retried as overload.
