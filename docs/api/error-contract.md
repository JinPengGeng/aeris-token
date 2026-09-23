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

Wallet denial is a permanent account state, not a temporary RPM window. All
client formats now share one unified insufficient-quota contract and never echo
the balance anywhere in the body:

| Client path | HTTP | Envelope |
| --- | --- | --- |
| OpenAI family (`/v1/chat/completions`, `/v1/responses`, `/v1/embeddings`, `/v1/images/*`, `/v1/rerank`, `/v1/videos`) | `429` | `{"error":{"message":"Insufficient quota","type":"insufficient_quota","param":null,"code":"insufficient_quota"}}` |
| Claude Messages (`/v1/messages`, `/v1/messages/count_tokens`) | `403` | `{"type":"error","error":{"type":"insufficient_quota","message":"Insufficient quota"}}`（无 `code` 字段） |
| Generic (Gemini/Codex Live/Antigravity 等未识别格式、图片预授权) | `429` | 与 OpenAI 信封一致 |

All variants retain `x-trace-id` for support correlation and omit
`Retry-After`（充值前重试无意义）. Neither format exposes the balance, user/key
identifiers, or internal billing details.

OpenAI 侧对齐 OpenAI 官方错误码（`code=insufficient_quota`）；Claude 侧对齐
Anthropic 官方语义（欠费是非 429、不可重试的错误）及 sub2api/new-api 的既有
做法（403 + Anthropic 信封）。这与上游 fawney19/Aether 的
`balance_exceeded`/`details.remaining` 泛化体是**刻意分叉**：余额数字不回显，
`balance_exceeded` 仅保留为入站上游错误类型的识别 marker（分类为
QuotaExhausted 后按上表重建信封），不再出现在任何对外响应中。

No `Retry-After` header is emitted for wallet denial or tenant permission
denial (`403/permission_error`). Provider rate limits remain `429/rate_limit_error`;
the gateway retains a provider-supplied wait time and does not invent one when
none is known. Daily usage, plan, and
RPM windows remain separate errors and include `Retry-After` plus their
existing `X-RateLimit-*` or `X-Daily-Usage-*` headers. Clients must not treat
`insufficient_quota` as a backoff-only event. Some SDKs retry
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
`error` object) and returns `403/insufficient_quota` for exhausted credit (see
the unified quota contract table above). Gemini
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
admission-overload builders use English messages. The unified
quota-exhausted contract (OpenAI-family `insufficient_quota`, Claude
`insufficient_quota`, and the generic fallback envelope) and the local
authentication/access-policy rejection messages negotiate English/Chinese via
the request `Accept-Language` header: the gateway defaults to English, and any
`zh` primary tag (for example `zh`, `zh-CN`, `zh-Hans`) selects Chinese
("Insufficient quota" / "余额不足"). Execution-runtime paths that no longer
have request headers in scope, execution diagnostics and provider-supplied
messages keep the existing language; the gateway does not provide broader
`Accept-Language` negotiation. Clients should branch on status and available
type/code fields, not translated message text.

The local JSON builders for those rejections, public resource errors, unknown
models and traced gateway dependency/timeouts include a top-level `trace_id`
matching `x-trace-id`. Dependency and timeout errors retain their existing nested
`error.trace_id` for compatibility. This is an additive local-error contract:
successful responses, provider body passthrough, SSE events and WebSocket frames
keep their protocol payloads. Use `x-trace-id` as the common correlation field
across all HTTP responses.
