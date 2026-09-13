# Aether gateway environment reference

This file is generated from `apps/aether-gateway/src`. Run
`python3 tools/check_gateway_env_reference.py --write` after changing an
environment variable. The checker fails when a source variable is missing from
this table or when the table contains a stale variable.

Clap-backed variables are available as command-line options on the default
server invocation and on `data export|import|copy|db ...` subcommands because
the data argument group is global. Hidden maintenance flags (`--migrate` and
`--apply-backfills`) are command-line-only and have no environment variable.

`unset` means the value is optional and the runtime default or auto-sizing
logic applies. Boolean values use clap's normal `true`/`false` parsing; flags
with `default_missing_value` can also be enabled by setting the variable to an
empty value only when explicitly documented in source. Secret values are
never printed by the checker.

| Variable | Source | Default | Unit / value |
| --- | --- | --- | --- |
| `AETHER_BACKUP_ENCRYPTION_KEY` | runtime-only | `unset` | string |
| `AETHER_BACKUP_HISTORICAL_KEYS_JSON` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_BACKUP_KEYRING_FILE` | clap (global; subcommands inherit) | `runtime (source-defined)` | count or enum (see source) |
| `AETHER_BARK_ALLOW_HTTP` | runtime-only | `unset` | boolean (true/false) |
| `AETHER_BARK_ALLOW_PRIVATE_TARGETS` | runtime-only | `unset` | boolean (true/false) |
| `AETHER_BASE_DIR` | runtime-only | `unset` | string |
| `AETHER_CODEX_WS_PROBE_ACCESS_TOKEN` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_CODEX_WS_PROBE_ACCOUNT_ID` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_CODEX_WS_PROBE_MODEL` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_CODEX_WS_PROBE_URL` | runtime-only | `unset` | string |
| `AETHER_DATABASE_DRIVER` | clap (global; subcommands inherit) | `unset` | count or enum (see source) |
| `AETHER_DATABASE_URL` | clap (global; subcommands inherit) | `unset` | string |
| `AETHER_GATEWAY_ADMIN_POOL_RUNTIME_WINDOW_METRIC_KEY_LIMIT` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_AUTH_CAPACITY_CACHE_TTL_MS` | runtime-only | `unset` | milliseconds |
| `AETHER_GATEWAY_AUTH_CONTEXT_CACHE_MAX_ENTRIES` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_AUTH_CONTEXT_CACHE_REFRESH_INTERVAL_SECS` | runtime-only | `unset` | seconds |
| `AETHER_GATEWAY_AUTH_CONTEXT_NEGATIVE_CACHE_TTL_SECS` | runtime-only | `unset` | seconds |
| `AETHER_GATEWAY_AUTH_SNAPSHOT_LOAD_GATE_LIMIT` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_AUTO_PREPARE_DATABASE` | clap (global; subcommands inherit) | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_BACKGROUND_DB_MAX_CONNECTIONS` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_CANDIDATE_PAGE_CACHE_STALE_TTL_MS` | runtime-only | `unset` | milliseconds |
| `AETHER_GATEWAY_CANDIDATE_PAGE_CACHE_TTL_MS` | runtime-only | `unset` | milliseconds |
| `AETHER_GATEWAY_CANDIDATE_PLANNING_GATE_LIMIT` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_DATABASE_MODE` | clap (global; subcommands inherit) | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_DATA_ENCRYPTION_KEY` | clap (global; subcommands inherit) | `unset` | string |
| `AETHER_GATEWAY_DATA_POSTGRES_ACQUIRE_TIMEOUT_MS` | clap (global; subcommands inherit) | `unset` | milliseconds |
| `AETHER_GATEWAY_DATA_POSTGRES_IDLE_TIMEOUT_MS` | clap (global; subcommands inherit) | `unset` | milliseconds |
| `AETHER_GATEWAY_DATA_POSTGRES_MAX_CONNECTIONS` | clap (global; subcommands inherit) | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_DATA_POSTGRES_MAX_LIFETIME_MS` | clap (global; subcommands inherit) | `unset` | milliseconds |
| `AETHER_GATEWAY_DATA_POSTGRES_MIN_CONNECTIONS` | clap (global; subcommands inherit) | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_DATA_POSTGRES_REQUIRE_SSL` | clap (global; subcommands inherit) | `unset` | boolean (true/false) |
| `AETHER_GATEWAY_DATA_POSTGRES_STATEMENT_CACHE_CAPACITY` | clap (global; subcommands inherit) | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_DATA_POSTGRES_URL` | clap (global; subcommands inherit) | `unset` | string |
| `AETHER_GATEWAY_DATA_REDIS_KEY_PREFIX` | clap (global; subcommands inherit) | `unset` | string |
| `AETHER_GATEWAY_DATA_REDIS_URL` | clap (global; subcommands inherit) | `unset` | string |
| `AETHER_GATEWAY_DEPLOYMENT_TOPOLOGY` | clap (global; subcommands inherit) | `single-node` | count or enum (see source) |
| `AETHER_GATEWAY_DIRECT_H2C_ADAPTIVE_WINDOW` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_DIRECT_H2C_CLIENT_SHARDS` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_DIRECT_H2C_DRIVER_RUNTIME_THREADS` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_DIRECT_H2C_FAST_PATH` | runtime-only | `unset` | string |
| `AETHER_GATEWAY_DIRECT_H2C_POOL_MAX_IDLE_PER_HOST` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_DIRECT_H2C_PREWARM_CONNECT_TIMEOUT_MS` | runtime-only | `unset` | milliseconds |
| `AETHER_GATEWAY_DIRECT_H2C_PREWARM_READY` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_DIRECT_H2C_PREWARM_URLS` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_DIRECT_H2C_SENDER_SELECT_WINDOW` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_DIRECT_H2C_TARGET_STREAMS_PER_CLIENT` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_DIRECT_PASSTHROUGH_CHANNEL_CAPACITY` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_DIRECT_PASSTHROUGH_MODE` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_DIRECT_REQWEST_CACHE_MAX_ENTRIES` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_DIRECT_REQWEST_CACHE_PER_ORIGIN` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_DIRECT_REQWEST_CLIENT_SHARDS` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_DIRECT_REQWEST_H2_CLIENT_SHARDS` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_DIRECT_REQWEST_H2_TARGET_STREAMS_PER_CLIENT` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_DIRECT_REQWEST_HTTP1_TARGET_STREAMS_PER_CLIENT` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_DIRECT_REQWEST_PREWARM_SYNC_CLIENTS` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_DIRECT_REQWEST_STREAM_HTTP_MODE` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_DIRECT_REQWEST_SYNC_WARM_CLIENTS` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_DISTRIBUTED_REQUEST_COMMAND_TIMEOUT_MS` | clap (global; subcommands inherit) | `1_000` | milliseconds |
| `AETHER_GATEWAY_DISTRIBUTED_REQUEST_LEASE_TTL_MS` | clap (global; subcommands inherit) | `30_000` | milliseconds |
| `AETHER_GATEWAY_DISTRIBUTED_REQUEST_LIMIT` | clap (global; subcommands inherit) | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_DISTRIBUTED_REQUEST_REDIS_KEY_PREFIX` | clap (global; subcommands inherit) | `unset` | string |
| `AETHER_GATEWAY_DISTRIBUTED_REQUEST_REDIS_URL` | clap (global; subcommands inherit) | `unset` | string |
| `AETHER_GATEWAY_DISTRIBUTED_REQUEST_RENEW_INTERVAL_MS` | clap (global; subcommands inherit) | `10_000` | milliseconds |
| `AETHER_GATEWAY_DISTRIBUTED_WEBSOCKET_CONNECTION_LIMIT` | clap (global; subcommands inherit) | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_ERROR_DETAIL_LOGGING` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_EXTERNAL_MODELS_URL` | runtime-only | `unset` | string |
| `AETHER_GATEWAY_HEALTHCHECK_TIMEOUT_MS` | clap (global; subcommands inherit) | `3_000` | milliseconds |
| `AETHER_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS` | clap (global; subcommands inherit) | `DEFAULT_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS` | count or enum (see source) |
| `AETHER_GATEWAY_HTTP_HEADER_MAX_BYTES` | clap (global; subcommands inherit) | `DEFAULT_GATEWAY_HTTP_HEADER_MAX_BYTES` | bytes / MiB (source-defined) |
| `AETHER_GATEWAY_HTTP_HEADER_READ_TIMEOUT_MS` | clap (global; subcommands inherit) | `DEFAULT_GATEWAY_HTTP_HEADER_READ_TIMEOUT_MS` | milliseconds |
| `AETHER_GATEWAY_HTTP_MAX_HEADERS` | clap (global; subcommands inherit) | `DEFAULT_GATEWAY_HTTP_MAX_HEADERS` | count or enum (see source) |
| `AETHER_GATEWAY_HTTP_SHUTDOWN_TIMEOUT_MS` | clap (global; subcommands inherit) | `30_000` | milliseconds |
| `AETHER_GATEWAY_INSTANCE_ID` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_INTERNAL_GATE_QUEUE_BUDGET_MS` | runtime-only | `unset` | milliseconds |
| `AETHER_GATEWAY_LISTENER_SHARDS` | clap (global; subcommands inherit) | `DEFAULT_GATEWAY_LISTENER_SHARDS` | count or enum (see source) |
| `AETHER_GATEWAY_LISTEN_BACKLOG` | clap (global; subcommands inherit) | `DEFAULT_GATEWAY_LISTEN_BACKLOG` | count or enum (see source) |
| `AETHER_GATEWAY_LOCAL_EXECUTION_PLANNING_TIMEOUT_MS` | runtime-only | `unset` | milliseconds |
| `AETHER_GATEWAY_MAX_HTTP_CONNECTIONS` | clap (global; subcommands inherit) | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_MAX_IN_FLIGHT_REQUESTS` | clap (global; subcommands inherit) | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_MAX_WEBSOCKET_CONNECTIONS` | clap (global; subcommands inherit) | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_NODE_ROLE` | clap (global; subcommands inherit) | `all` | count or enum (see source) |
| `AETHER_GATEWAY_OPENAI_CHAT_STREAM_TARGET_SELECT_WINDOW` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_POOL_SCORE_FAILURE_FEEDBACK_MIN_INTERVAL_SECS` | runtime-only | `unset` | seconds |
| `AETHER_GATEWAY_POOL_SCORE_SUCCESS_FEEDBACK_MIN_INTERVAL_SECS` | runtime-only | `unset` | seconds |
| `AETHER_GATEWAY_PROVIDER_KEY_ADAPTIVE_SUCCESS_PERSIST_MIN_INTERVAL_SECS` | runtime-only | `unset` | seconds |
| `AETHER_GATEWAY_PROVIDER_KEY_HEALTH_SUCCESS_PERSIST_MIN_INTERVAL_SECS` | runtime-only | `unset` | seconds |
| `AETHER_GATEWAY_PROVIDER_POOL_IN_FLIGHT_ACQUIRE_TIMEOUT_MS` | runtime-only | `unset` | milliseconds |
| `AETHER_GATEWAY_PROVIDER_POOL_IN_FLIGHT_MODE` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_READINESS_WITHDRAWAL_DELAY_MS` | clap (global; subcommands inherit) | `2_000` | milliseconds |
| `AETHER_GATEWAY_REQUEST_BODY_BUFFER_BUDGET_MB` | runtime-only | `unset` | bytes / MiB (source-defined) |
| `AETHER_GATEWAY_REQUEST_BODY_READ_TIMEOUT_MS` | runtime-only | `unset` | milliseconds |
| `AETHER_GATEWAY_REQUEST_CANDIDATE_DB_BATCH_SIZE` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_REQUEST_CANDIDATE_DB_WRITE_CONCURRENCY_LIMIT` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_REQUEST_CANDIDATE_PERSISTENCE` | runtime-only | `unset` | boolean (true/false) |
| `AETHER_GATEWAY_REQUEST_CANDIDATE_QUEUE_BATCH_SIZE` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_REQUEST_CANDIDATE_QUEUE_CAPACITY` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_REQUEST_CANDIDATE_QUEUE_FLUSH_INTERVAL_MS` | runtime-only | `unset` | milliseconds |
| `AETHER_GATEWAY_REQUEST_CANDIDATE_QUEUE_FULL` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_REQUEST_CANDIDATE_QUEUE_RUNTIME_THREADS` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_REQUEST_CANDIDATE_QUEUE_WORKERS` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_REQUEST_CANDIDATE_SEED_WRITE_TIMEOUT_MS` | runtime-only | `unset` | milliseconds |
| `AETHER_GATEWAY_REQUEST_CANDIDATE_WRITE_MODE` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_SECURITY_CACHE_TTL_MS` | runtime-only | `unset` | milliseconds |
| `AETHER_GATEWAY_STAGE_METRICS_ENABLED` | runtime-only | `unset` | boolean (true/false) |
| `AETHER_GATEWAY_STAGE_TRACE_MODE` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_STAGE_TRACE_SAMPLE_RATE` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_STAGE_TRACE_SLOW_MS` | runtime-only | `unset` | milliseconds |
| `AETHER_GATEWAY_STATIC_DIR` | clap (global; subcommands inherit) | `unset` | string |
| `AETHER_GATEWAY_STREAM_CAPTURE_MEMORY_BUDGET_BYTES` | runtime-only | `unset` | bytes / MiB (source-defined) |
| `AETHER_GATEWAY_TRUSTED_INGRESS_CIDRS` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_UPSTREAM_EXECUTION_GATE_HOLD_STREAM_RESPONSE` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_UPSTREAM_EXECUTION_GATE_LIMIT` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_UPSTREAM_EXECUTION_GATE_STREAM_HOLD_MODE` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_UPSTREAM_POOL_IDLE_TIMEOUT_MS` | runtime-only | `unset` | milliseconds |
| `AETHER_GATEWAY_UPSTREAM_POOL_MAX_IDLE_PER_HOST` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_UPSTREAM_STREAM_IDLE_TIMEOUT_MS` | runtime-only | `unset` | milliseconds |
| `AETHER_GATEWAY_UPSTREAM_TARGET_GATE_LIMIT` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_UPSTREAM_TARGET_GATE_METRIC_LIMIT` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_UPSTREAM_TARGET_GATE_QUEUE_BUDGET_MS` | runtime-only | `unset` | milliseconds |
| `AETHER_GATEWAY_USAGE_COUNTER_DELTA_CLEANUP_BATCH_SIZE` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_USAGE_COUNTER_DELTA_CLEANUP_INTERVAL_MS` | runtime-only | `unset` | milliseconds |
| `AETHER_GATEWAY_USAGE_COUNTER_DELTA_RETENTION_SECS` | runtime-only | `unset` | seconds |
| `AETHER_GATEWAY_USAGE_COUNTER_FLUSH_BATCH_SIZE` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_USAGE_COUNTER_FLUSH_CATCH_UP_BURST_LIMIT` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_USAGE_COUNTER_FLUSH_INTERVAL_MS` | runtime-only | `unset` | milliseconds |
| `AETHER_GATEWAY_USAGE_ENQUEUE_RETRY_BUFFER_CAPACITY` | clap (global; subcommands inherit) | `131_072` | count or enum (see source) |
| `AETHER_GATEWAY_USAGE_ENQUEUE_RETRY_INITIAL_BACKOFF_MS` | clap (global; subcommands inherit) | `3_000` | milliseconds |
| `AETHER_GATEWAY_USAGE_ENQUEUE_RETRY_MAX_BACKOFF_MS` | clap (global; subcommands inherit) | `10_000` | milliseconds |
| `AETHER_GATEWAY_USAGE_ENQUEUE_RETRY_WORKERS` | clap (global; subcommands inherit) | `8` | count or enum (see source) |
| `AETHER_GATEWAY_USAGE_LIFECYCLE_ENQUEUE_DELAY_MS` | clap (global; subcommands inherit) | `1_000` | milliseconds |
| `AETHER_GATEWAY_USAGE_LIFECYCLE_ENQUEUE_MAX_IN_FLIGHT` | clap (global; subcommands inherit) | `512` | count or enum (see source) |
| `AETHER_GATEWAY_USAGE_QUEUE_BATCH_SIZE` | clap (global; subcommands inherit) | `128` | count or enum (see source) |
| `AETHER_GATEWAY_USAGE_QUEUE_BLOCK_MS` | clap (global; subcommands inherit) | `500` | milliseconds |
| `AETHER_GATEWAY_USAGE_QUEUE_DLQ_MAXLEN` | clap (global; subcommands inherit) | `50_000` | count or enum (see source) |
| `AETHER_GATEWAY_USAGE_QUEUE_DLQ_STREAM_KEY` | clap (global; subcommands inherit) | `usage:events:dlq` | string |
| `AETHER_GATEWAY_USAGE_QUEUE_GROUP` | clap (global; subcommands inherit) | `usage_consumers` | count or enum (see source) |
| `AETHER_GATEWAY_USAGE_QUEUE_LIFECYCLE_EVENTS` | clap (global; subcommands inherit) | `true` | count or enum (see source) |
| `AETHER_GATEWAY_USAGE_QUEUE_PAYLOAD_MAX_BYTES` | clap (global; subcommands inherit) | `1024 * 1024` | bytes / MiB (source-defined) |
| `AETHER_GATEWAY_USAGE_QUEUE_RECLAIM_COUNT` | clap (global; subcommands inherit) | `128` | count or enum (see source) |
| `AETHER_GATEWAY_USAGE_QUEUE_RECLAIM_IDLE_MS` | clap (global; subcommands inherit) | `60_000` | milliseconds |
| `AETHER_GATEWAY_USAGE_QUEUE_RECLAIM_INTERVAL_MS` | clap (global; subcommands inherit) | `5_000` | milliseconds |
| `AETHER_GATEWAY_USAGE_QUEUE_STREAM_KEY` | clap (global; subcommands inherit) | `usage:events` | string |
| `AETHER_GATEWAY_USAGE_QUEUE_STREAM_MAXLEN` | clap (global; subcommands inherit) | `200_000` | count or enum (see source) |
| `AETHER_GATEWAY_USAGE_QUEUE_TERMINAL_EVENTS` | clap (global; subcommands inherit) | `true` | count or enum (see source) |
| `AETHER_GATEWAY_USAGE_QUEUE_WORKERS` | clap (global; subcommands inherit) | `unset` | count or enum (see source) |
| `AETHER_GATEWAY_USAGE_QUEUE_WORKER_AUTOSCALE_ENABLED` | clap (global; subcommands inherit) | `true` | boolean (true/false) |
| `AETHER_GATEWAY_USAGE_QUEUE_WORKER_IDLE_SCALE_DOWN_TICKS` | clap (global; subcommands inherit) | `30` | count or enum (see source) |
| `AETHER_GATEWAY_USAGE_QUEUE_WORKER_MAX_COUNT` | clap (global; subcommands inherit) | `32` | count or enum (see source) |
| `AETHER_GATEWAY_USAGE_QUEUE_WORKER_SCALE_INTERVAL_MS` | clap (global; subcommands inherit) | `1_000` | milliseconds |
| `AETHER_GATEWAY_USAGE_RETRY_DEFERRED_LIFECYCLE_EVENTS` | clap (global; subcommands inherit) | `true` | count or enum (see source) |
| `AETHER_GATEWAY_USAGE_SHUTDOWN_TIMEOUT_MS` | clap (global; subcommands inherit) | `30_000` | milliseconds |
| `AETHER_GATEWAY_USAGE_TERMINAL_ENQUEUE_MAX_IN_FLIGHT` | clap (global; subcommands inherit) | `1_024` | count or enum (see source) |
| `AETHER_GATEWAY_USAGE_TERMINAL_SUBMISSION_MAX_IN_FLIGHT` | clap (global; subcommands inherit) | `1_024` | count or enum (see source) |
| `AETHER_GATEWAY_USAGE_WORKER_RECORD_CONCURRENCY_LIMIT` | clap (global; subcommands inherit) | `32` | count or enum (see source) |
| `AETHER_GATEWAY_VIDEO_TASK_POLLER_BATCH_SIZE` | clap (global; subcommands inherit) | `32` | count or enum (see source) |
| `AETHER_GATEWAY_VIDEO_TASK_POLLER_INTERVAL_MS` | clap (global; subcommands inherit) | `5000` | milliseconds |
| `AETHER_GATEWAY_VIDEO_TASK_STORE_PATH` | clap (global; subcommands inherit) | `unset` | string |
| `AETHER_GATEWAY_VIDEO_TASK_TRUTH_SOURCE_MODE` | clap (global; subcommands inherit) | `python-sync-report` | count or enum (see source) |
| `AETHER_INTERNAL_GATEWAY_AUTH_SECRET` | runtime-only | `unset` | string |
| `AETHER_LOG_DESTINATION` | clap (global; subcommands inherit) | `stdout` | count or enum (see source) |
| `AETHER_LOG_DIR` | clap (global; subcommands inherit) | `unset` | string |
| `AETHER_LOG_FORMAT` | clap (global; subcommands inherit) | `pretty` | count or enum (see source) |
| `AETHER_LOG_MAX_FILES` | clap (global; subcommands inherit) | `30` | count or enum (see source) |
| `AETHER_LOG_RETENTION_DAYS` | clap (global; subcommands inherit) | `7` | count or enum (see source) |
| `AETHER_LOG_ROTATION` | clap (global; subcommands inherit) | `daily` | count or enum (see source) |
| `AETHER_MAX_INTERNAL_BUFFERED_BODY_MB` | runtime-only | `unset` | bytes / MiB (source-defined) |
| `AETHER_MAX_REDACTED_SYNC_RESPONSE_BODY_MB` | runtime-only | `unset` | bytes / MiB (source-defined) |
| `AETHER_MAX_REQUEST_BODY_MB` | runtime-only | `unset` | bytes / MiB (source-defined) |
| `AETHER_OPENAI_WS_PROBE_API_KEY` | runtime-only | `unset` | string |
| `AETHER_OPENAI_WS_PROBE_MODEL` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_OPENAI_WS_PROBE_URL` | runtime-only | `unset` | string |
| `AETHER_PUBLIC_BASE_URL` | runtime-only | `unset` | string |
| `AETHER_RUNTIME_BACKEND` | clap (global; subcommands inherit) | `unset` | count or enum (see source) |
| `AETHER_RUNTIME_COMMAND_TIMEOUT_MS` | clap (global; subcommands inherit) | `2_000` | milliseconds |
| `AETHER_RUNTIME_REDIS_KEY_PREFIX` | clap (global; subcommands inherit) | `unset` | string |
| `AETHER_RUNTIME_REDIS_URL` | clap (global; subcommands inherit) | `unset` | string |
| `AETHER_TRUSTED_PROXY_CIDRS` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_TUNNEL_ATTACHMENT_TTL_SECS` | runtime-only | `unset` | seconds |
| `AETHER_TUNNEL_BASE_URL` | runtime-only | `unset` | string |
| `AETHER_TUNNEL_DRAIN_DEADLINE_MS` | runtime-only | `unset` | milliseconds |
| `AETHER_TUNNEL_NODE_STATUS_QUEUE_CAPACITY` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_TUNNEL_RELAY_ALLOW_PRIVATE_TARGETS` | runtime-only | `unset` | boolean (true/false) |
| `AETHER_TUNNEL_RELAY_AUTH_SECRET` | runtime-only | `unset` | string |
| `AETHER_TUNNEL_RELAY_BASE_URL` | runtime-only | `unset` | string |
| `AETHER_TUNNEL_RELAY_MAX_BODY_MB` | runtime-only | `unset` | bytes / MiB (source-defined) |
| `AETHER_TUNNEL_RELAY_PRIVATE_HOST_ALLOWLIST` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_TUNNEL_RELAY_SPOOL_BUDGET_MB` | runtime-only | `unset` | bytes / MiB (source-defined) |
| `AETHER_TUNNEL_STREAM_INITIAL_WINDOW_BYTES` | runtime-only | `unset` | bytes / MiB (source-defined) |
| `AETHER_UPDATE_STRATEGY` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_VSCODEX_ENABLED` | runtime-only | `unset` | boolean (true/false) |
| `AETHER_VSCODEX_INTERNAL_TOKEN` | runtime-only | `unset` | count or enum (see source) |
| `AETHER_VSCODEX_INTERNAL_URL` | runtime-only | `unset` | string |
| `APP_PORT` | clap (global; subcommands inherit) | `8084` | count or enum (see source) |
| `APP_TIMEZONE` | runtime-only | `unset` | count or enum (see source) |
| `AUTH_REFRESH_COOKIE_NAME` | runtime-only | `unset` | count or enum (see source) |
| `AUTH_REFRESH_COOKIE_SAMESITE` | runtime-only | `unset` | count or enum (see source) |
| `AUTH_REFRESH_COOKIE_SECURE` | runtime-only | `unset` | boolean (true/false) |
| `BATCH_BALANCE_CONCURRENCY` | runtime-only | `unset` | count or enum (see source) |
| `CODEIUM_API_URL` | runtime-only | `unset` | string |
| `CORS_ALLOW_CREDENTIALS` | clap (global; subcommands inherit) | `true` | boolean (true/false) |
| `CORS_ORIGINS` | clap (global; subcommands inherit) | `unset` | count or enum (see source) |
| `DATABASE_URL` | runtime-only | `unset` | string |
| `ENCRYPTION_KEY` | runtime-only | `unset` | string |
| `ENVIRONMENT` | clap (global; subcommands inherit) | `development` | count or enum (see source) |
| `HOME` | runtime-only | `unset` | count or enum (see source) |
| `HOSTNAME` | runtime-only | `unset` | count or enum (see source) |
| `JWT_EXPIRATION_HOURS` | runtime-only | `unset` | count or enum (see source) |
| `JWT_SECRET_KEY` | runtime-only | `unset` | string |
| `MANAGEMENT_TOKEN_MAX_PER_USER` | runtime-only | `unset` | count or enum (see source) |
| `PAYMENT_CALLBACK_SECRET` | runtime-only | `unset` | string |
| `PUBLIC_BASE_URL` | runtime-only | `unset` | string |
| `RATE_LIMIT_FAIL_OPEN` | clap (global; subcommands inherit) | `false` | boolean (true/false) |
| `REDIS_URL` | runtime-only | `unset` | string |
| `RPM_BUCKET_SECONDS` | clap (global; subcommands inherit) | `60` | seconds |
| `RPM_KEY_TTL_SECONDS` | clap (global; subcommands inherit) | `120` | seconds |
| `USERPROFILE` | runtime-only | `unset` | count or enum (see source) |
| `VERIFICATION_CODE_EXPIRE_MINUTES` | runtime-only | `unset` | count or enum (see source) |
| `VERIFICATION_SEND_COOLDOWN` | runtime-only | `unset` | count or enum (see source) |
