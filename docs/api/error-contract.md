# Public API Error Contract

This document records the error contract at the public gateway boundary. It
describes the current fork implementation, not every provider's native error
format. `x-trace-id` is present on gateway responses for correlation.

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
| 503/529 | `server_error` | Retry with bounded backoff when `Retry-After` is present. |

## Quota versus rate limit

Wallet denial is a permanent account state, not a temporary RPM window. For
OpenAI-family routes it remains HTTP `429` for compatibility with clients that
recognize OpenAI's quota response, but has this stable payload:

```json
{
  "error": {
    "message": "Insufficient quota",
    "type": "rate_limit_error",
    "code": "insufficient_quota"
  }
}
```

No `Retry-After` header is emitted for wallet denial. Daily usage, plan, and
RPM windows remain separate errors and include `Retry-After` plus their
existing `X-RateLimit-*` or `X-Daily-Usage-*` headers. Clients must not treat
`insufficient_quota` as a backoff-only event.

## Other public formats

Claude Messages uses the Anthropic envelope (`type=error` with a nested
`error` object), and Gemini uses its `error.code/message/status` envelope.
Changing the OpenAI contract does not rewrite these format-specific errors.
Provider error bodies are not copied blindly across the public boundary;
non-client provider failures are projected to a generic gateway error to avoid
credential or internal URL disclosure.

## Conversion and model errors

Cross-format conversion is fail-closed. A field with no audited lossless
mapping returns a structured conversion error instead of being silently
dropped. Unknown public models return HTTP `404` with
`error.code=model_not_found`. These outcomes are permanent request/configuration
failures and must not be retried as overload.
