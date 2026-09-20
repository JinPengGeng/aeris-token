# Videos API

Aether exposes an OpenAI video task surface under `/v1/videos` and the
OpenAI-compatible xAI adapter path `/openai/v1/videos`. Requests use the
`openai:video` client format and require an Aether key that is allowed to use
both that format and the requested model.

Video generation is asynchronous. A create request returns a task record; poll
the record until it reaches a terminal status, then retrieve its content when
available. The gateway stores task ownership and only exposes a task to the
user that created it.

## Supported endpoints

| Method | Path | Behavior |
| --- | --- | --- |
| `POST` | `/v1/videos` | Create a video task. |
| `GET` | `/v1/videos/{id}` | Read a task's current state. |
| `DELETE` | `/v1/videos/{id}` | Delete a task. |
| `POST` | `/v1/videos/{id}/cancel` | Cancel a task that can still be cancelled. |
| `POST` | `/v1/videos/{id}/remix` | Create a remix from an existing task. |
| `GET` | `/v1/videos/{id}/content` | Return the generated video bytes when content is available. |

The same task operations are accepted below `/openai/v1/videos`. The explicit
native xAI create paths `POST /v1/videos/generations`, `POST
/v1/videos/edits`, and `POST /v1/videos/extensions` are usable only when a
selected xAI provider is configured. They are not general OpenAI video aliases.

## Create and poll

```bash
curl -sS "http://localhost:8084/v1/videos" \
  -H "Authorization: Bearer sk-your-aether-key" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "sora-2",
    "prompt": "A red paper boat crosses a quiet blue lake"
  }'
```

The request body is forwarded in the selected video provider's supported
format after model mapping. Provider/model-specific fields are not a gateway
promise: configure a matching provider and use only fields it supports. An
xAI provider accepts the native xAI request shape on the native paths. On
`/openai/v1/videos`, Aether adapts the OpenAI-compatible request for xAI.

Poll the returned task `id`:

```bash
curl -sS "http://localhost:8084/v1/videos/task_123" \
  -H "Authorization: Bearer sk-your-aether-key"
```

Task records expose their state, including `queued`, `processing`, `completed`,
`failed`, or `cancelled`, and can include progress and a video URL when the
provider has produced one. Do not assume the create request holds open until
generation completes: this API does not use SSE for task progress.

After completion, download content through the task route:

```bash
curl -L "http://localhost:8084/v1/videos/task_123/content" \
  -H "Authorization: Bearer sk-your-aether-key" \
  --output video.mp4
```

## Errors and limits

Gateway-generated errors use the OpenAI-family envelope and every gateway
response includes `x-trace-id`:

```json
{"error":{"message":"...","type":"invalid_request_error"},"trace_id":"trace-..."}
```

Typical statuses are `400` for invalid requests, `401` for missing or invalid
credentials, `403` for a format or access-policy denial, `404` for a task that
does not exist or is not owned by the caller, `429` for a rate or quota limit,
and `503` for temporary provider or gateway unavailability. A caller cannot use
another user's task ID to read, cancel, remix, delete, or download a task; the
gateway returns `404` without forwarding that task to a provider.

Video requests are subject to the API key's model and `openai:video` format
permissions plus the deployment's provider availability and usage limits. The
gateway does not document a universal duration, resolution, prompt, or task
count limit because those depend on the configured provider/model and account
policy. For general authentication, quota, rate-limit, retry, and correlation
behavior, see the [public error contract](error-contract.md).
