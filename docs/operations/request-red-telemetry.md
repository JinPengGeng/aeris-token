# Request RED and log/trace telemetry contract

Status: accepted contract for Issue #306 (217-C/E).  This document fixes the
dimensions and collection semantics used by request telemetry.  It does not
install Prometheus, Grafana, Alertmanager, or an OpenTelemetry SDK.

## Dimensions and budget

The same bounded values MUST be used by request counters and latency samples.
Unknown values are preferred over creating a new label at runtime.

| Dimension | Allowed values | Maximum cardinality | Source and default |
| --- | --- | ---: | --- |
| `route_class` | `admin_proxy`, `ai_public`, `auth`, `internal_proxy`, `local`, `passthrough`, `public_support`, `unknown` | 8 | trusted control decision; `local` for a response without a decision, otherwise `unknown` |
| `status_class` | `1xx`, `2xx`, `3xx`, `4xx`, `5xx`, `unknown` | 6 | terminal HTTP status; `unknown` is reserved for a non-HTTP/cancelled outcome |
| `provider_type` | `openai`, `codex`, `chatgpt_web`, `claude_code`, `kiro`, `grok`, `gemini_cli`, `antigravity`, `windsurf`, `vertex_ai`, `custom`, `other`, `unknown` | 13 | trusted execution response header; `unknown` when no provider attempt was made |

The implementation lives in `aether-gateway-frontdoor::telemetry`.  Provider
IDs, API-key IDs, model names, raw paths, user IDs, error text, trace IDs and
retry/attempt numbers are never metric labels.  A provider value not in the
allowlist is collapsed to `unknown`.

`status_code` may be present in an access log for debugging, but only
`status_class` is a RED dimension.  The sanitized request path may also be
logged; query credentials are removed by the existing path sanitizer.

## Request and stream semantics

The request lifecycle has one terminal observation.  A retry or failover of an
upstream provider does not create another external request total; it is an
internal attempt.  The future metrics implementation must therefore increment
the terminal request counter exactly once after the client response status is
known.  The outcome mapping is:

* `2xx`/`3xx`: successful terminal response;
* `4xx`: client/auth/policy rejection (still a completed request);
* `5xx`: gateway or upstream failure;
* cancellation before a response: a separate bounded cancellation counter and
  no fabricated HTTP status.

For a streaming response, `latency_first_byte` is measured from front-door
acceptance until the first non-empty body frame.  `latency_terminal` is measured
until the body closes.  A stream that fails before its first frame records an
upstream error with no first-byte observation; a stream that fails after a first
frame keeps the original status and records the terminal failure event.  These
rules make first-byte and terminal latency queries comparable without exposing
body contents.

## Trace and access-log fields

The front door accepts `x-trace-id` or generates a UUID, forwards it to trusted
handlers, and echoes it on the response.  Access and audit records use the same
opaque value.  The value is a correlation key only: it must never contain an
Authorization header, API key, cookie, provider credential, request body, or
raw error text.  Request IDs are shortened before logging.

The terminal access event uses these stable fields:

```text
event_name=http_request_completed|http_request_failed
log_type=access
trace_id=<opaque correlation id>
route_class=<bounded value>
status_class=<bounded value>
provider_type=<bounded value>
status_code=<HTTP status, log-only>
elapsed_ms=<non-negative integer>
```

`http_request_started` is diagnostic and has no status class.  A structured
JSON line contains the same fields under the runtime formatter's `fields`
object; pretty output renders them as `key=value` pairs.  Both forms preserve
the event names and field names so collectors can migrate without parsing the
human-readable message.

## Pretty/JSON compatibility and migration

Pretty remains the default to preserve local development and existing
deployments.  Select structured output explicitly:

```sh
AETHER_LOG_FORMAT=json docker compose up gateway
```

The compose files already expose `AETHER_LOG_FORMAT` and retain
`${AETHER_LOG_FORMAT:-pretty}`.  A collector migration should:

1. deploy a canary with `AETHER_LOG_FORMAT=json` and parse one JSON object per
   line;
2. verify `event_name`, `trace_id`, `route_class`, `status_class` and
   `provider_type` against this contract;
3. roll out to the remaining instances while retaining the same trace field;
4. roll back by setting `AETHER_LOG_FORMAT=pretty` and restarting the service.

No monitoring stack is required.  File sinks use the same format setting as
stdout, and rotation/retention settings are unchanged.

## Verification and residual scope

Focused front-door tests cover bounded route/provider labels, all HTTP status
ranges, trace propagation, path credential redaction, JSON access events and
pretty/JSON runtime formatter parsing.  Billing/fail-open counters and alert
rules are tracked by #307; dependency-aware readiness is tracked by #308.
