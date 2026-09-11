# Images API

Aether exposes OpenAI-compatible image generation and edit surfaces:

| Method | Path | Client format |
| --- | --- | --- |
| `POST` | `/v1/images/generations` | `openai:image` |
| `POST` | `/v1/images/edits` | `openai:image` |

The API key must be allowed to use the selected image model and image API
format. Image provider availability, model capability, and pricing are
validated before a request is sent.

## Generate

```bash
curl -sS "http://localhost:8084/v1/images/generations" \
  -H "Authorization: Bearer sk-your-aether-key" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-image-2",
    "prompt": "A red paper boat on a blue lake",
    "n": 1,
    "response_format": "b64_json"
  }'
```

`model` and `prompt` are required. Aether supports `n`, `size`, `quality`,
`background`, `moderation`, `output_format`, `output_compression`,
`response_format`, and `partial_images` where the selected provider/model
supports them. `partial_images` is only valid with streaming and accepts
values from 0 through 3. The gateway-wide generation count limit is applied
before provider model aliases are resolved.

## Edit

JSON edits use an `images` array containing image URLs or data URLs:

```bash
curl -sS "http://localhost:8084/v1/images/edits" \
  -H "Authorization: Bearer sk-your-aether-key" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-image-2",
    "prompt": "Turn the sky into a sunset",
    "images": ["data:image/png;base64,<base64-image>"]
  }'
```

Multipart edits accept `model`, `prompt`, and one or more `image`/`images`
file parts. Multipart boundaries, part count, header size, duplicate names,
and truncated bodies are rejected before provider transport.

## Response And Errors

Successful responses use the OpenAI image response shape (`data[]`, with
`url` or `b64_json` according to `response_format`). Streaming responses use
OpenAI-compatible SSE image events.

Validation failures return HTTP `400` and the OpenAI error envelope. Messages
are English and identify the invalid field. Unsupported `style`, invalid
`quality`/`background`/`moderation`/`input_fidelity`, invalid output format,
and missing prompt or edit images are deterministic client errors. See
`error-contract.md` for status, retry, quota, and model-not-found semantics.
