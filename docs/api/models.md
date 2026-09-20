# Models API

List the models visible to the authenticated API key, or inspect one model.
These routes use the API format implied by the request. OpenAI clients use
the OpenAI model-list envelope.

## List models

```bash
curl -sS "http://localhost:8084/v1/models" \
  -H "Authorization: Bearer sk-your-aether-key"
```

Typical response:

```json
{"object":"list","data":[{"id":"gpt-5","object":"model","created":0,"owned_by":"aether"}]}
```

The list is filtered by the key's allowed models, providers, and API formats.
An empty list is a valid response when no permitted model is configured.

## Model detail

```bash
curl -sS "http://localhost:8084/v1/models/gpt-5" \
  -H "Authorization: Bearer sk-your-aether-key"
```

An unknown or inaccessible model returns HTTP `404` with
`error.code=model_not_found`. Missing or invalid authentication returns `401`.
Responses and Codex clients may request the Responses-shaped catalog; the
route remains `GET /v1/models` and returns the catalog shape required by that
client.
