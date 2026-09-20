# Messages API

Aether exposes the Anthropic-compatible Messages surface at `POST
/v1/messages`. The gateway authenticates the request, selects an allowed model
and provider candidate, and preserves the Claude Messages request and response
shape for the `claude:messages` client format.

## Authentication

Send an Aether API key in `x-api-key` (or `api-key`). The route classifier also
accepts a Bearer credential carrier for compatible clients. Include
`anthropic-version` when using an Anthropic SDK or another client that requires
that header; the gateway forwards only the headers appropriate for the selected
provider.

The key must allow both the requested model and the `claude:messages` API
format. Public model names are mapped to provider model names by the gateway
configuration.

## Create a message

```bash
curl -sS "http://localhost:8084/v1/messages" \
  -H "x-api-key: sk-your-aether-key" \
  -H "anthropic-version: 2023-06-01" \
  -H "content-type: application/json" \
  -d '{
    "model": "claude-sonnet-4-5",
    "max_tokens": 128,
    "messages": [{"role": "user", "content": "Say hello in one sentence."}]
  }'
```

Use the Anthropic Messages request object. The model must be visible to the
key. Fields supported by a selected same-format provider are preserved; when
routing requires format conversion, a field without an audited lossless mapping
fails closed instead of being silently discarded. See the
[format conversion audit](format-conversion-audit.md).

## Streaming

Set `stream` to `true` to receive Anthropic-compatible server-sent events:

```bash
curl -N -sS "http://localhost:8084/v1/messages" \
  -H "x-api-key: sk-your-aether-key" \
  -H "anthropic-version: 2023-06-01" \
  -H "content-type: application/json" \
  -d '{"model":"claude-sonnet-4-5","max_tokens":128,"messages":[{"role":"user","content":"Stream a short greeting."}],"stream":true}'
```

The response content type is `text/event-stream`. A rejection before the stream
is committed is an HTTP JSON error; after commitment the HTTP status cannot
change. When both the client and selected provider use `claude:messages`, a
gateway terminal error uses Anthropic `event: error`. On a format-conversion
route, terminal errors currently use a generic JSON `data:` event followed by
`data: [DONE]`; clients using those routes must account for that difference.

## Count tokens

`POST /v1/messages/count_tokens` is supported as a non-streaming Anthropic
token-count operation. Its JSON body must contain a non-empty string `model`
and an array `messages`; it returns the Anthropic token-count response and does
not generate a message.

```bash
curl -sS "http://localhost:8084/v1/messages/count_tokens" \
  -H "x-api-key: sk-your-aether-key" \
  -H "anthropic-version: 2023-06-01" \
  -H "content-type: application/json" \
  -d '{"model":"claude-sonnet-4-5","messages":[{"role":"user","content":"Count these tokens."}]}'
```

## Errors and limits

Claude Messages uses the Anthropic error envelope:

```json
{"type":"error","error":{"type":"invalid_request_error","message":"..."},"trace_id":"trace-..."}
```

Gateway responses include `x-trace-id`. Typical statuses are `400` for invalid
input, `401` for missing or invalid credentials, `403` for model or API-format
policy denial, `404` for a missing resource/model, `429` for provider rate
limits, and `503` for transient provider or gateway unavailability. A wallet
denial is distinct: it is `402` with `error.type=billing_error` and
`error.code=balance_exceeded`, has message `Insufficient quota`, and has no
`Retry-After`; restore credit instead of retrying unchanged requests.

Provider rate limits retain a provider-supplied wait time when known. Daily
usage, plan, and RPM limits include their existing rate-limit headers and
`Retry-After`. See the [public error contract](error-contract.md) for retry
guidance and error-language boundaries.
