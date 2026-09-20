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
  },
  "trace_id": "trace-..."
}
```

The `error.code` field is included when a stable code is available. The
gateway's generic mapping is:

| HTTP status | `error.type` | Retry guidance |
| --- | --- | --- |
| 400/405/422 | `invalid_request_error` | Fix the request; do not retry unchanged. |
| 401 | `authentication_error` | Replace credentials. |
| 403 | `permission_error` | Change policy, key, or model access. |
| 404 | `not_found_error` | Check the path/resource; model-detail lookup uses `model_not_found`. See the inference distinction below. |
| 413 | `context_length_exceeded` | Reduce request size. |
| 429 | `rate_limit_error` | Retry only when the response identifies a transient rate/quota window. |
| 429 | `insufficient_quota` | Restore account credit; do not retry unchanged. |
| 503/529 | `server_error` | For transient unavailability, use bounded backoff and honor `Retry-After`. Admission overload responses always include `Retry-After: 1`; empty candidate lists can also reflect configuration problems; see below. |

Gateway-generated local execution and admission diagnostics use English messages. This does not alter upstream response bodies relayed by the gateway.

## Missing or invalid credentials

At the public AI authentication boundary, missing credentials or credential
carriers that the existing extractor cannot accept produce the same HTTP
`401` rejection as an unknown API key. OpenAI-family routes use
`authentication_error`, retain `x-trace-id`, and omit `Retry-After`. This applies
to both normal requests and streaming requests rejected before commitment.
Provide accepted credentials before retrying; an unchanged anonymous request
does not become valid through backoff.

Request admission runs before authentication. If the configured distributed
request gate cannot acquire a Redis lease, its existing `503/server_error`
response includes `Retry-After: 1` and can therefore take precedence even for a
request without credentials.
This does not reclassify missing internal execution/authentication context as
a client error, or change deferred cookie/Google Bearer resolution.

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
  },
  "trace_id": "trace-..."
}
```

Claude Messages wallet denial uses HTTP `402` and
`{"type":"error","error":{"type":"billing_error","code":"balance_exceeded","message":"Insufficient quota"},"trace_id":"trace-..."}`.
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
  },
  "trace_id": "trace-..."
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
dropped. Correct the request or select a provider with an audited mapping;
retrying the same unsupported conversion does not resolve the problem.

Model-detail lookup (`GET /v1/models/:id`) returns HTTP `404` with
`error.code=model_not_found` when no visible model matches. Authenticated Chat
and Images inference apply the same distinction after local candidate selection fails:
an absent public global-model record returns `404/not_found_error` with
`model_not_found`, while a declared model with no selectable provider remains
retryable `503/server_error`. The classifier runs after public authentication
and access-policy rejection, so anonymous or unauthorized callers receive
their existing `401`/`403` response instead of model-directory information.

The classifier also checks scheduler-declared canonical names and model aliases
before returning `404`, so a declared but currently unselectable alias remains
`503`. If the public model directory or scheduler declaration read is unavailable,
the gateway preserves the existing `503` runtime miss rather than treating an
operational lookup failure as absence. Other inference route families retain
their separate acceptance coverage.

## Message language and correlation

Local request-validation, authentication, access-policy, wallet, usage-limit and
admission-overload builders use English messages. Execution diagnostics and
provider-supplied messages can use other languages; the gateway does not provide
`Accept-Language` negotiation. Clients should branch on status and available
type/code fields, not translated message text.

The local JSON builders for those rejections, public resource errors, unknown
models and traced gateway dependency/timeouts include a top-level `trace_id`
matching `x-trace-id`. Dependency and timeout errors retain their existing nested
`error.trace_id` for compatibility. This is an additive local-error contract:
successful responses, provider body passthrough, SSE events and WebSocket frames
keep their protocol payloads. Use `x-trace-id` as the common correlation field
across all HTTP responses.
