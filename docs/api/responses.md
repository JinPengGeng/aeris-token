# Responses API

Aether exposes the OpenAI Responses API at `POST /v1/responses`. The gateway
uses the model and API key permissions to select an allowed provider. Send the
same `input` shape used by OpenAI; `model` is required.

## Quick Start

```bash
curl -sS "http://localhost:8084/v1/responses" \
  -H "Authorization: Bearer sk-your-aether-key" \
  -H "Content-Type: application/json" \
  -d '{"model":"gpt-5","input":"Say hello in one sentence."}'
```

The key must allow the requested model and the `openai:responses` API format.
The public model name is the name in the request; provider mappings may use a
different upstream model name.

## Streaming

Set `stream` to `true` to receive OpenAI-compatible server-sent events. The
response content type is `text/event-stream`.

```bash
curl -N -sS "http://localhost:8084/v1/responses" \
  -H "Authorization: Bearer sk-your-aether-key" \
  -H "Content-Type: application/json" \
  -d '{"model":"gpt-5","input":"Stream a short greeting.","stream":true}'
```

## Request and compacting

`input` may be a string or an array of Responses input items. Common optional
fields include `instructions`, `tools`, `tool_choice`, `temperature`,
`max_output_tokens`, `reasoning`, `text`, `metadata`, `store`, and
`previous_response_id`, subject to the selected provider's lossless mapping.
Unsupported cross-format fields fail closed instead of being silently dropped.

For clients that use OpenAI's compaction method, the same authentication
surface is available at `POST /v1/responses/compact` with a body containing at
least `model` and, when needed, `input` or `previous_response_id`:

```bash
curl -sS "http://localhost:8084/v1/responses/compact" \
  -H "Authorization: Bearer sk-your-aether-key" \
  -H "Content-Type: application/json" \
  -d '{"model":"gpt-5","input":"Earlier context to compact."}'
```

## Errors

Responses errors use the OpenAI envelope:

```json
{"error":{"message":"...","type":"invalid_request_error"}}
```

Typical statuses are `400` for malformed or incomplete input, `401` for a
missing or invalid key, `403` for a model/API-format policy denial, `404` for
a missing model, `429` for quota or rate limits, and `503` for provider or
gateway unavailability. Every gateway response includes `x-trace-id` for
correlation. An exhausted wallet is `429` with
`error.type=insufficient_quota` and `error.code=credit_balance_exhausted`; it
is not retryable and has no `Retry-After` header. See the [public error
contract](error-contract.md).
