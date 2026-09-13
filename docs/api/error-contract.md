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
| 404 | `not_found_error` | Check the path/resource; model-detail lookup uses `model_not_found`. See the inference distinction below. |
| 413 | `context_length_exceeded` | Reduce request size. |
| 429 | `rate_limit_error` | Retry only when the response identifies a transient rate/quota window. |
| 429 | `insufficient_quota` | Restore account credit; do not retry unchanged. |
| 503/529 | `server_error` | For transient unavailability, use bounded backoff and honor `Retry-After` when present. Empty candidate lists can also reflect configuration problems; see below. |

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
response can therefore take precedence even for a request without credentials.
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
dropped. Correct the request or select a provider with an audited mapping;
retrying the same unsupported conversion does not resolve the problem.

Model-detail lookup (`GET /v1/models/:id`) returns HTTP `404` with
`error.code=model_not_found` when no visible model matches. Inference POST
requests do not yet distinguish all unknown-model cases from an empty candidate
list caused by unavailable providers or configuration. That path can still
return HTTP `503`; the existing Chat/Images model-not-found fixture rows are
target contracts, not proof of inference route behavior. This classification
gap remains tracked in Issue #254. Clients should check the requested model
and provider configuration, and use bounded retries only when the cause is
transient.

## Message language and correlation

Local image-field validation and exhausted-credit messages are English. Other
authentication, access-policy and execution messages may still be Chinese;
the gateway does not provide `Accept-Language` negotiation for this contract.
Clients should branch on status and available type/code fields, not translated
message text. `x-trace-id` is the correlation contract; a `trace_id` body field
is present only on some error paths and must not be assumed for every response.
