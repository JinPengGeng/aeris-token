# Chat Completions API

Aether exposes an OpenAI-compatible chat surface at `POST /v1/chat/completions`.
The gateway authenticates the request, selects an allowed model and provider
candidate, and preserves the OpenAI Chat request and response shape for the
`openai:chat` client format.

## Quick Start

```bash
curl -sS "http://localhost:8084/v1/chat/completions" \
  -H "Authorization: Bearer sk-your-aether-key" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-5",
    "messages": [{"role": "user", "content": "Say hello in one sentence."}]
  }'
```

The key must be allowed to use the requested model and the `openai:chat`
client API format. The model catalog and provider mapping are configured by an
administrator; a provider model name may differ from the public global model
name.

## Request

The gateway accepts the OpenAI Chat Completions request object. `model` and
`messages` are required. Standard fields such as `stream`, `temperature`,
`tools`, `tool_choice`, `response_format`, `reasoning_effort`, and `user` are
handled by the conversion layer when the selected provider supports them.
Fields with no lossless target mapping fail closed during cross-format
routing. Same-format provider extensions are preserved according to
`format-passthrough-contract.md`.

For streaming requests, set `stream` to `true`. The response is
`text/event-stream`; the gateway adds `Cache-Control: no-cache, no-transform`
and `X-Accel-Buffering: no` so an intermediate proxy does not buffer events.

## Response

Non-streaming requests return the OpenAI Chat Completions response from the
selected provider. Streaming requests return OpenAI-compatible SSE events and
finish with the provider's terminal usage/finish event when available.

Every response includes `x-trace-id` for support and log correlation. The
gateway does not expose provider redirect or security-sensitive response
headers.

## Failure Behavior

Local validation errors use the OpenAI envelope:

```json
{
  "error": {
    "message": "...",
    "type": "invalid_request_error"
  }
}
```

The status and error type are deterministic: `400` is invalid input,
`401` is authentication failure, `403` is an access policy denial, `404` is a
missing resource/model, `429` is a retryable rate or quota limit, and `503` is
provider/gateway overload. A missing public model is reported as
`404` with `error.code=model_not_found`.

An exhausted wallet is not a rate-limit retry signal even though the HTTP
status remains `429` for compatibility with OpenAI clients. It is returned as
`error.type=rate_limit_error`, `error.code=insufficient_quota`, and the
message `Insufficient quota`; it has no `Retry-After` header. Clients should
stop retrying and restore quota instead of applying exponential backoff.

For conversion failures, the gateway fails closed rather than silently
dropping fields. See `format-conversion-audit.md` for the supported mapping
and `error-contract.md` for the complete public error matrix.
