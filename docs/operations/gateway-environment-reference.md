# Aether gateway environment reference

This source reference covers literal clap declarations and recognized runtime
environment readers under `apps/aether-gateway/src`; it does not enumerate
environment settings in dependencies or deployment scripts. It never reads or
prints configured environment values.

Regenerate with `python3 tools/check_gateway_env_reference.py --write`.
The check compares the complete generated document, including declared
defaults, Rust types, scope and clap validation attributes. Unsupported clap
env declarations fail instead of being silently omitted. Required Rust CI runs
the checker and its regression tests.

Only arguments explicitly marked global are inherited by `data` subcommands.
Root/server arguments are not inherited; the standalone backup-restore binary
has its own arguments. `unset` denotes an `Option` without a declared default;
it does not mean zero or disabled. Rust default expressions below are preserved
as source expressions, not evaluated by this tool. Empty environment values
are not a general way to enable boolean options.

## Clap declarations

| Variable | Scope | Declared default | Rust type | Source |
| --- | --- | --- | --- | --- |
| `AETHER_BACKUP_KEYRING_FILE` | standalone aether-backup-restore CLI | `unset` | `Option<PathBuf>` | [bin/aether-backup-restore.rs](../../apps/aether-gateway/src/bin/aether-backup-restore.rs) |
| `AETHER_DATABASE_DRIVER` | gateway global; inherited by data subcommands | `unset` | `Option<DatabaseDriverArg>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_DATABASE_URL` | gateway global; inherited by data subcommands | `unset` | `Option<String>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_AUTO_PREPARE_DATABASE` | gateway root/server; not inherited by subcommands | `unset` | `Option<bool>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_DATABASE_MODE` | gateway root/server; not inherited by subcommands | `unset` | `Option<DatabaseModeArg>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_DATA_ENCRYPTION_KEY` | gateway global; inherited by data subcommands | `unset` | `Option<String>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_DATA_POSTGRES_ACQUIRE_TIMEOUT_MS` | gateway global; inherited by data subcommands | `unset` | `Option<u64>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_DATA_POSTGRES_IDLE_TIMEOUT_MS` | gateway global; inherited by data subcommands | `unset` | `Option<u64>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_DATA_POSTGRES_MAX_CONNECTIONS` | gateway global; inherited by data subcommands | `unset` | `Option<u32>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_DATA_POSTGRES_MAX_LIFETIME_MS` | gateway global; inherited by data subcommands | `unset` | `Option<u64>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_DATA_POSTGRES_MIN_CONNECTIONS` | gateway global; inherited by data subcommands | `unset` | `Option<u32>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_DATA_POSTGRES_REQUIRE_SSL` | gateway global; inherited by data subcommands | `unset` | `Option<bool>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_DATA_POSTGRES_STATEMENT_CACHE_CAPACITY` | gateway global; inherited by data subcommands | `unset` | `Option<usize>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_DATA_POSTGRES_URL` | gateway global; inherited by data subcommands | `unset` | `Option<String>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_DATA_REDIS_KEY_PREFIX` | gateway global; inherited by data subcommands | `unset` | `Option<String>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_DATA_REDIS_URL` | gateway global; inherited by data subcommands | `unset` | `Option<String>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_DEPLOYMENT_TOPOLOGY` | gateway root/server; not inherited by subcommands | `"single-node"` | `DeploymentTopologyArg` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_DISTRIBUTED_REQUEST_COMMAND_TIMEOUT_MS` | gateway root/server; not inherited by subcommands | `1_000` | `u64` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_DISTRIBUTED_REQUEST_LEASE_TTL_MS` | gateway root/server; not inherited by subcommands | `30_000` | `u64` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_DISTRIBUTED_REQUEST_LIMIT` | gateway root/server; not inherited by subcommands | `unset` | `Option<usize>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_DISTRIBUTED_REQUEST_REDIS_KEY_PREFIX` | gateway root/server; not inherited by subcommands | `unset` | `Option<String>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_DISTRIBUTED_REQUEST_REDIS_URL` | gateway root/server; not inherited by subcommands | `unset` | `Option<String>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_DISTRIBUTED_REQUEST_RENEW_INTERVAL_MS` | gateway root/server; not inherited by subcommands | `10_000` | `u64` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_DISTRIBUTED_WEBSOCKET_CONNECTION_LIMIT` | gateway root/server; not inherited by subcommands | `unset` | `Option<usize>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_HEALTHCHECK_TIMEOUT_MS` | gateway root/server; not inherited by subcommands | `3_000` | `u64` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS` | gateway root/server; not inherited by subcommands | `DEFAULT_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS` | `u32` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_HTTP_HEADER_MAX_BYTES` | gateway root/server; not inherited by subcommands | `DEFAULT_GATEWAY_HTTP_HEADER_MAX_BYTES` | `usize` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_HTTP_HEADER_READ_TIMEOUT_MS` | gateway root/server; not inherited by subcommands | `DEFAULT_GATEWAY_HTTP_HEADER_READ_TIMEOUT_MS` | `u64` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_HTTP_MAX_HEADERS` | gateway root/server; not inherited by subcommands | `DEFAULT_GATEWAY_HTTP_MAX_HEADERS` | `usize` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_HTTP_SHUTDOWN_TIMEOUT_MS` | gateway root/server; not inherited by subcommands | `30_000` | `u64` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_LISTENER_SHARDS` | gateway root/server; not inherited by subcommands | `DEFAULT_GATEWAY_LISTENER_SHARDS` | `usize` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_LISTEN_BACKLOG` | gateway root/server; not inherited by subcommands | `DEFAULT_GATEWAY_LISTEN_BACKLOG` | `i32` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_MAX_HTTP_CONNECTIONS` | gateway root/server; not inherited by subcommands | `unset` | `Option<usize>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_MAX_IN_FLIGHT_REQUESTS` | gateway root/server; not inherited by subcommands | `unset` | `Option<usize>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_MAX_WEBSOCKET_CONNECTIONS` | gateway root/server; not inherited by subcommands | `unset` | `Option<usize>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_NODE_ROLE` | gateway root/server; not inherited by subcommands | `"all"` | `NodeRoleArg` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_READINESS_WITHDRAWAL_DELAY_MS` | gateway root/server; not inherited by subcommands | `2_000` | `u64` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_STATIC_DIR` | gateway root/server; not inherited by subcommands | `unset` | `Option<String>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_ENQUEUE_RETRY_BUFFER_CAPACITY` | gateway root/server; not inherited by subcommands | `131_072` | `usize` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_ENQUEUE_RETRY_INITIAL_BACKOFF_MS` | gateway root/server; not inherited by subcommands | `3_000` | `u64` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_ENQUEUE_RETRY_MAX_BACKOFF_MS` | gateway root/server; not inherited by subcommands | `10_000` | `u64` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_ENQUEUE_RETRY_WORKERS` | gateway root/server; not inherited by subcommands | `8` | `usize` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_LIFECYCLE_ENQUEUE_DELAY_MS` | gateway root/server; not inherited by subcommands | `1_000` | `u64` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_LIFECYCLE_ENQUEUE_MAX_IN_FLIGHT` | gateway root/server; not inherited by subcommands | `512` | `u64` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_QUEUE_BATCH_SIZE` | gateway root/server; not inherited by subcommands | `128` | `usize` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_QUEUE_BLOCK_MS` | gateway root/server; not inherited by subcommands | `500` | `u64` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_QUEUE_DLQ_MAXLEN` | gateway root/server; not inherited by subcommands | `50_000` | `usize` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_QUEUE_DLQ_STREAM_KEY` | gateway root/server; not inherited by subcommands | `"usage:events:dlq"` | `String` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_QUEUE_GROUP` | gateway root/server; not inherited by subcommands | `"usage_consumers"` | `String` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_QUEUE_LIFECYCLE_EVENTS` | gateway root/server; not inherited by subcommands | `true` | `bool` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_QUEUE_PAYLOAD_MAX_BYTES` | gateway root/server; not inherited by subcommands | `1024 * 1024` | `usize` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_QUEUE_RECLAIM_COUNT` | gateway root/server; not inherited by subcommands | `128` | `usize` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_QUEUE_RECLAIM_IDLE_MS` | gateway root/server; not inherited by subcommands | `60_000` | `u64` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_QUEUE_RECLAIM_INTERVAL_MS` | gateway root/server; not inherited by subcommands | `5_000` | `u64` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_QUEUE_STREAM_KEY` | gateway root/server; not inherited by subcommands | `"usage:events"` | `String` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_QUEUE_STREAM_MAXLEN` | gateway root/server; not inherited by subcommands | `200_000` | `usize` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_QUEUE_TERMINAL_EVENTS` | gateway root/server; not inherited by subcommands | `true` | `bool` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_QUEUE_WORKERS` | gateway root/server; not inherited by subcommands | `unset` | `Option<usize>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_QUEUE_WORKER_AUTOSCALE_ENABLED` | gateway root/server; not inherited by subcommands | `true` | `bool` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_QUEUE_WORKER_IDLE_SCALE_DOWN_TICKS` | gateway root/server; not inherited by subcommands | `30` | `u64` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_QUEUE_WORKER_MAX_COUNT` | gateway root/server; not inherited by subcommands | `"32"` | `Option<usize>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_QUEUE_WORKER_SCALE_INTERVAL_MS` | gateway root/server; not inherited by subcommands | `1_000` | `u64` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_RETRY_DEFERRED_LIFECYCLE_EVENTS` | gateway root/server; not inherited by subcommands | `true` | `bool` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_SHUTDOWN_TIMEOUT_MS` | gateway root/server; not inherited by subcommands | `30_000` | `u64` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_TERMINAL_ENQUEUE_MAX_IN_FLIGHT` | gateway root/server; not inherited by subcommands | `1_024` | `u64` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_TERMINAL_SUBMISSION_MAX_IN_FLIGHT` | gateway root/server; not inherited by subcommands | `1_024` | `u64` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_USAGE_WORKER_RECORD_CONCURRENCY_LIMIT` | gateway root/server; not inherited by subcommands | `"32"` | `Option<usize>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_VIDEO_TASK_POLLER_BATCH_SIZE` | gateway root/server; not inherited by subcommands | `32` | `usize` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_VIDEO_TASK_POLLER_INTERVAL_MS` | gateway root/server; not inherited by subcommands | `5000` | `u64` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_VIDEO_TASK_STORE_PATH` | gateway root/server; not inherited by subcommands | `unset` | `Option<String>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_GATEWAY_VIDEO_TASK_TRUTH_SOURCE_MODE` | gateway root/server; not inherited by subcommands | `"python-sync-report"` | `VideoTaskTruthSourceArg` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_LOG_DESTINATION` | gateway root/server; not inherited by subcommands | `"stdout"` | `GatewayLogDestinationArg` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_LOG_DIR` | gateway root/server; not inherited by subcommands | `unset` | `Option<String>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_LOG_FORMAT` | gateway root/server; not inherited by subcommands | `"pretty"` | `GatewayLogFormatArg` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_LOG_MAX_FILES` | gateway root/server; not inherited by subcommands | `30` | `usize` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_LOG_RETENTION_DAYS` | gateway root/server; not inherited by subcommands | `7` | `u64` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_LOG_ROTATION` | gateway root/server; not inherited by subcommands | `"daily"` | `GatewayLogRotationArg` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_RUNTIME_BACKEND` | gateway root/server; not inherited by subcommands | `unset` | `Option<RuntimeBackendArg>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_RUNTIME_COMMAND_TIMEOUT_MS` | gateway root/server; not inherited by subcommands | `2_000` | `u64` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_RUNTIME_REDIS_KEY_PREFIX` | gateway root/server; not inherited by subcommands | `unset` | `Option<String>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `AETHER_RUNTIME_REDIS_URL` | gateway root/server; not inherited by subcommands | `unset` | `Option<String>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `APP_PORT` | gateway root/server; not inherited by subcommands | `8084` | `u16` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `CORS_ALLOW_CREDENTIALS` | gateway root/server; not inherited by subcommands | `true` | `bool` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `CORS_ORIGINS` | gateway root/server; not inherited by subcommands | `unset` | `Option<String>` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `ENVIRONMENT` | gateway root/server; not inherited by subcommands | `"development"` | `String` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `RATE_LIMIT_FAIL_OPEN` | gateway root/server; not inherited by subcommands | `false` | `bool` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `RPM_BUCKET_SECONDS` | gateway root/server; not inherited by subcommands | `60` | `u64` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `RPM_KEY_TTL_SECONDS` | gateway root/server; not inherited by subcommands | `120` | `u64` | [main.rs](../../apps/aether-gateway/src/main.rs) |

### Declared parsing and validation

These declarations preserve value parsers and missing-value handling for review.
They contain names and source defaults, not current environment values.

- `AETHER_BACKUP_KEYRING_FILE`: `long, env = "AETHER_BACKUP_KEYRING_FILE"`
- `AETHER_DATABASE_DRIVER`: `long, env = "AETHER_DATABASE_DRIVER", global = true`
- `AETHER_DATABASE_URL`: `long, env = "AETHER_DATABASE_URL", global = true`
- `AETHER_GATEWAY_AUTO_PREPARE_DATABASE`: `long, env = "AETHER_GATEWAY_AUTO_PREPARE_DATABASE", hide = true, num_args = 0..=1, default_missing_value = "true"`
- `AETHER_GATEWAY_DATABASE_MODE`: `long, env = "AETHER_GATEWAY_DATABASE_MODE", value_enum`
- `AETHER_GATEWAY_DATA_ENCRYPTION_KEY`: `long, env = "AETHER_GATEWAY_DATA_ENCRYPTION_KEY", global = true`
- `AETHER_GATEWAY_DATA_POSTGRES_ACQUIRE_TIMEOUT_MS`: `long, env = "AETHER_GATEWAY_DATA_POSTGRES_ACQUIRE_TIMEOUT_MS", global = true`
- `AETHER_GATEWAY_DATA_POSTGRES_IDLE_TIMEOUT_MS`: `long, env = "AETHER_GATEWAY_DATA_POSTGRES_IDLE_TIMEOUT_MS", global = true`
- `AETHER_GATEWAY_DATA_POSTGRES_MAX_CONNECTIONS`: `long, env = "AETHER_GATEWAY_DATA_POSTGRES_MAX_CONNECTIONS", global = true`
- `AETHER_GATEWAY_DATA_POSTGRES_MAX_LIFETIME_MS`: `long, env = "AETHER_GATEWAY_DATA_POSTGRES_MAX_LIFETIME_MS", global = true`
- `AETHER_GATEWAY_DATA_POSTGRES_MIN_CONNECTIONS`: `long, env = "AETHER_GATEWAY_DATA_POSTGRES_MIN_CONNECTIONS", global = true`
- `AETHER_GATEWAY_DATA_POSTGRES_REQUIRE_SSL`: `long, env = "AETHER_GATEWAY_DATA_POSTGRES_REQUIRE_SSL", num_args = 0..=1, default_missing_value = "true", global = true`
- `AETHER_GATEWAY_DATA_POSTGRES_STATEMENT_CACHE_CAPACITY`: `long, env = "AETHER_GATEWAY_DATA_POSTGRES_STATEMENT_CACHE_CAPACITY", global = true`
- `AETHER_GATEWAY_DATA_POSTGRES_URL`: `long, env = "AETHER_GATEWAY_DATA_POSTGRES_URL", global = true`
- `AETHER_GATEWAY_DATA_REDIS_KEY_PREFIX`: `long, env = "AETHER_GATEWAY_DATA_REDIS_KEY_PREFIX", global = true`
- `AETHER_GATEWAY_DATA_REDIS_URL`: `long, env = "AETHER_GATEWAY_DATA_REDIS_URL", global = true`
- `AETHER_GATEWAY_DEPLOYMENT_TOPOLOGY`: `long, env = "AETHER_GATEWAY_DEPLOYMENT_TOPOLOGY", value_enum, default_value = "single-node"`
- `AETHER_GATEWAY_DISTRIBUTED_REQUEST_COMMAND_TIMEOUT_MS`: `long, env = "AETHER_GATEWAY_DISTRIBUTED_REQUEST_COMMAND_TIMEOUT_MS", default_value_t = 1_000`
- `AETHER_GATEWAY_DISTRIBUTED_REQUEST_LEASE_TTL_MS`: `long, env = "AETHER_GATEWAY_DISTRIBUTED_REQUEST_LEASE_TTL_MS", default_value_t = 30_000`
- `AETHER_GATEWAY_DISTRIBUTED_REQUEST_LIMIT`: `long, env = "AETHER_GATEWAY_DISTRIBUTED_REQUEST_LIMIT"`
- `AETHER_GATEWAY_DISTRIBUTED_REQUEST_REDIS_KEY_PREFIX`: `long, env = "AETHER_GATEWAY_DISTRIBUTED_REQUEST_REDIS_KEY_PREFIX"`
- `AETHER_GATEWAY_DISTRIBUTED_REQUEST_REDIS_URL`: `long, env = "AETHER_GATEWAY_DISTRIBUTED_REQUEST_REDIS_URL"`
- `AETHER_GATEWAY_DISTRIBUTED_REQUEST_RENEW_INTERVAL_MS`: `long, env = "AETHER_GATEWAY_DISTRIBUTED_REQUEST_RENEW_INTERVAL_MS", default_value_t = 10_000`
- `AETHER_GATEWAY_DISTRIBUTED_WEBSOCKET_CONNECTION_LIMIT`: `long, env = "AETHER_GATEWAY_DISTRIBUTED_WEBSOCKET_CONNECTION_LIMIT"`
- `AETHER_GATEWAY_HEALTHCHECK_TIMEOUT_MS`: `long, hide = true, env = "AETHER_GATEWAY_HEALTHCHECK_TIMEOUT_MS", default_value_t = 3_000`
- `AETHER_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS`: `long, env = "AETHER_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS", default_value_t = DEFAULT_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS`
- `AETHER_GATEWAY_HTTP_HEADER_MAX_BYTES`: `long, env = "AETHER_GATEWAY_HTTP_HEADER_MAX_BYTES", default_value_t = DEFAULT_GATEWAY_HTTP_HEADER_MAX_BYTES`
- `AETHER_GATEWAY_HTTP_HEADER_READ_TIMEOUT_MS`: `long, env = "AETHER_GATEWAY_HTTP_HEADER_READ_TIMEOUT_MS", default_value_t = DEFAULT_GATEWAY_HTTP_HEADER_READ_TIMEOUT_MS`
- `AETHER_GATEWAY_HTTP_MAX_HEADERS`: `long, env = "AETHER_GATEWAY_HTTP_MAX_HEADERS", default_value_t = DEFAULT_GATEWAY_HTTP_MAX_HEADERS`
- `AETHER_GATEWAY_HTTP_SHUTDOWN_TIMEOUT_MS`: `long, env = "AETHER_GATEWAY_HTTP_SHUTDOWN_TIMEOUT_MS", default_value_t = 30_000`
- `AETHER_GATEWAY_LISTENER_SHARDS`: `long, env = "AETHER_GATEWAY_LISTENER_SHARDS", default_value_t = DEFAULT_GATEWAY_LISTENER_SHARDS`
- `AETHER_GATEWAY_LISTEN_BACKLOG`: `long, env = "AETHER_GATEWAY_LISTEN_BACKLOG", default_value_t = DEFAULT_GATEWAY_LISTEN_BACKLOG`
- `AETHER_GATEWAY_MAX_HTTP_CONNECTIONS`: `long, env = "AETHER_GATEWAY_MAX_HTTP_CONNECTIONS"`
- `AETHER_GATEWAY_MAX_IN_FLIGHT_REQUESTS`: `long, env = "AETHER_GATEWAY_MAX_IN_FLIGHT_REQUESTS"`
- `AETHER_GATEWAY_MAX_WEBSOCKET_CONNECTIONS`: `long, env = "AETHER_GATEWAY_MAX_WEBSOCKET_CONNECTIONS"`
- `AETHER_GATEWAY_NODE_ROLE`: `long, env = "AETHER_GATEWAY_NODE_ROLE", value_enum, default_value = "all"`
- `AETHER_GATEWAY_READINESS_WITHDRAWAL_DELAY_MS`: `long, env = "AETHER_GATEWAY_READINESS_WITHDRAWAL_DELAY_MS", default_value_t = 2_000, value_parser = clap::value_parser!(u64).range(0..=60_000)`
- `AETHER_GATEWAY_STATIC_DIR`: `long, env = "AETHER_GATEWAY_STATIC_DIR"`
- `AETHER_GATEWAY_USAGE_ENQUEUE_RETRY_BUFFER_CAPACITY`: `long, env = "AETHER_GATEWAY_USAGE_ENQUEUE_RETRY_BUFFER_CAPACITY", default_value_t = 131_072`
- `AETHER_GATEWAY_USAGE_ENQUEUE_RETRY_INITIAL_BACKOFF_MS`: `long, env = "AETHER_GATEWAY_USAGE_ENQUEUE_RETRY_INITIAL_BACKOFF_MS", default_value_t = 3_000`
- `AETHER_GATEWAY_USAGE_ENQUEUE_RETRY_MAX_BACKOFF_MS`: `long, env = "AETHER_GATEWAY_USAGE_ENQUEUE_RETRY_MAX_BACKOFF_MS", default_value_t = 10_000`
- `AETHER_GATEWAY_USAGE_ENQUEUE_RETRY_WORKERS`: `long, env = "AETHER_GATEWAY_USAGE_ENQUEUE_RETRY_WORKERS", default_value_t = 8`
- `AETHER_GATEWAY_USAGE_LIFECYCLE_ENQUEUE_DELAY_MS`: `long, env = "AETHER_GATEWAY_USAGE_LIFECYCLE_ENQUEUE_DELAY_MS", default_value_t = 1_000`
- `AETHER_GATEWAY_USAGE_LIFECYCLE_ENQUEUE_MAX_IN_FLIGHT`: `long, env = "AETHER_GATEWAY_USAGE_LIFECYCLE_ENQUEUE_MAX_IN_FLIGHT", default_value_t = 512`
- `AETHER_GATEWAY_USAGE_QUEUE_BATCH_SIZE`: `long, env = "AETHER_GATEWAY_USAGE_QUEUE_BATCH_SIZE", default_value_t = 128`
- `AETHER_GATEWAY_USAGE_QUEUE_BLOCK_MS`: `long, env = "AETHER_GATEWAY_USAGE_QUEUE_BLOCK_MS", default_value_t = 500`
- `AETHER_GATEWAY_USAGE_QUEUE_DLQ_MAXLEN`: `long, env = "AETHER_GATEWAY_USAGE_QUEUE_DLQ_MAXLEN", default_value_t = 50_000`
- `AETHER_GATEWAY_USAGE_QUEUE_DLQ_STREAM_KEY`: `long, env = "AETHER_GATEWAY_USAGE_QUEUE_DLQ_STREAM_KEY", default_value = "usage:events:dlq"`
- `AETHER_GATEWAY_USAGE_QUEUE_GROUP`: `long, env = "AETHER_GATEWAY_USAGE_QUEUE_GROUP", default_value = "usage_consumers"`
- `AETHER_GATEWAY_USAGE_QUEUE_LIFECYCLE_EVENTS`: `long, env = "AETHER_GATEWAY_USAGE_QUEUE_LIFECYCLE_EVENTS", default_value_t = true`
- `AETHER_GATEWAY_USAGE_QUEUE_PAYLOAD_MAX_BYTES`: `long, env = "AETHER_GATEWAY_USAGE_QUEUE_PAYLOAD_MAX_BYTES", default_value_t = 1024 * 1024`
- `AETHER_GATEWAY_USAGE_QUEUE_RECLAIM_COUNT`: `long, env = "AETHER_GATEWAY_USAGE_QUEUE_RECLAIM_COUNT", default_value_t = 128`
- `AETHER_GATEWAY_USAGE_QUEUE_RECLAIM_IDLE_MS`: `long, env = "AETHER_GATEWAY_USAGE_QUEUE_RECLAIM_IDLE_MS", default_value_t = 60_000`
- `AETHER_GATEWAY_USAGE_QUEUE_RECLAIM_INTERVAL_MS`: `long, env = "AETHER_GATEWAY_USAGE_QUEUE_RECLAIM_INTERVAL_MS", default_value_t = 5_000`
- `AETHER_GATEWAY_USAGE_QUEUE_STREAM_KEY`: `long, env = "AETHER_GATEWAY_USAGE_QUEUE_STREAM_KEY", default_value = "usage:events"`
- `AETHER_GATEWAY_USAGE_QUEUE_STREAM_MAXLEN`: `long, env = "AETHER_GATEWAY_USAGE_QUEUE_STREAM_MAXLEN", default_value_t = 200_000`
- `AETHER_GATEWAY_USAGE_QUEUE_TERMINAL_EVENTS`: `long, env = "AETHER_GATEWAY_USAGE_QUEUE_TERMINAL_EVENTS", default_value_t = true`
- `AETHER_GATEWAY_USAGE_QUEUE_WORKERS`: `long, env = "AETHER_GATEWAY_USAGE_QUEUE_WORKERS", value_name = "COUNT"`
- `AETHER_GATEWAY_USAGE_QUEUE_WORKER_AUTOSCALE_ENABLED`: `long, env = "AETHER_GATEWAY_USAGE_QUEUE_WORKER_AUTOSCALE_ENABLED", default_value_t = true`
- `AETHER_GATEWAY_USAGE_QUEUE_WORKER_IDLE_SCALE_DOWN_TICKS`: `long, env = "AETHER_GATEWAY_USAGE_QUEUE_WORKER_IDLE_SCALE_DOWN_TICKS", default_value_t = 30`
- `AETHER_GATEWAY_USAGE_QUEUE_WORKER_MAX_COUNT`: `long, env = "AETHER_GATEWAY_USAGE_QUEUE_WORKER_MAX_COUNT", value_name = "COUNT", default_value = "32"`
- `AETHER_GATEWAY_USAGE_QUEUE_WORKER_SCALE_INTERVAL_MS`: `long, env = "AETHER_GATEWAY_USAGE_QUEUE_WORKER_SCALE_INTERVAL_MS", default_value_t = 1_000`
- `AETHER_GATEWAY_USAGE_RETRY_DEFERRED_LIFECYCLE_EVENTS`: `long, env = "AETHER_GATEWAY_USAGE_RETRY_DEFERRED_LIFECYCLE_EVENTS", default_value_t = true`
- `AETHER_GATEWAY_USAGE_SHUTDOWN_TIMEOUT_MS`: `long, env = "AETHER_GATEWAY_USAGE_SHUTDOWN_TIMEOUT_MS", default_value_t = 30_000`
- `AETHER_GATEWAY_USAGE_TERMINAL_ENQUEUE_MAX_IN_FLIGHT`: `long, env = "AETHER_GATEWAY_USAGE_TERMINAL_ENQUEUE_MAX_IN_FLIGHT", default_value_t = 1_024`
- `AETHER_GATEWAY_USAGE_TERMINAL_SUBMISSION_MAX_IN_FLIGHT`: `long, env = "AETHER_GATEWAY_USAGE_TERMINAL_SUBMISSION_MAX_IN_FLIGHT", default_value_t = 1_024`
- `AETHER_GATEWAY_USAGE_WORKER_RECORD_CONCURRENCY_LIMIT`: `long, env = "AETHER_GATEWAY_USAGE_WORKER_RECORD_CONCURRENCY_LIMIT", value_name = "COUNT", default_value = "32"`
- `AETHER_GATEWAY_VIDEO_TASK_POLLER_BATCH_SIZE`: `long, env = "AETHER_GATEWAY_VIDEO_TASK_POLLER_BATCH_SIZE", default_value_t = 32`
- `AETHER_GATEWAY_VIDEO_TASK_POLLER_INTERVAL_MS`: `long, env = "AETHER_GATEWAY_VIDEO_TASK_POLLER_INTERVAL_MS", default_value_t = 5000`
- `AETHER_GATEWAY_VIDEO_TASK_STORE_PATH`: `long, env = "AETHER_GATEWAY_VIDEO_TASK_STORE_PATH"`
- `AETHER_GATEWAY_VIDEO_TASK_TRUTH_SOURCE_MODE`: `long, env = "AETHER_GATEWAY_VIDEO_TASK_TRUTH_SOURCE_MODE", value_enum, default_value = "python-sync-report"`
- `AETHER_LOG_DESTINATION`: `long, env = "AETHER_LOG_DESTINATION", value_enum, default_value = "stdout"`
- `AETHER_LOG_DIR`: `long, env = "AETHER_LOG_DIR"`
- `AETHER_LOG_FORMAT`: `long, env = "AETHER_LOG_FORMAT", value_enum, default_value = "pretty"`
- `AETHER_LOG_MAX_FILES`: `long, env = "AETHER_LOG_MAX_FILES", default_value_t = 30`
- `AETHER_LOG_RETENTION_DAYS`: `long, env = "AETHER_LOG_RETENTION_DAYS", default_value_t = 7`
- `AETHER_LOG_ROTATION`: `long, env = "AETHER_LOG_ROTATION", value_enum, default_value = "daily"`
- `AETHER_RUNTIME_BACKEND`: `long, env = "AETHER_RUNTIME_BACKEND", value_enum`
- `AETHER_RUNTIME_COMMAND_TIMEOUT_MS`: `long, env = "AETHER_RUNTIME_COMMAND_TIMEOUT_MS", default_value_t = 2_000`
- `AETHER_RUNTIME_REDIS_KEY_PREFIX`: `long, env = "AETHER_RUNTIME_REDIS_KEY_PREFIX"`
- `AETHER_RUNTIME_REDIS_URL`: `long, env = "AETHER_RUNTIME_REDIS_URL"`
- `APP_PORT`: `long, env = "APP_PORT", default_value_t = 8084`
- `CORS_ALLOW_CREDENTIALS`: `long, env = "CORS_ALLOW_CREDENTIALS", default_value_t = true`
- `CORS_ORIGINS`: `long, env = "CORS_ORIGINS"`
- `ENVIRONMENT`: `long, env = "ENVIRONMENT", default_value = "development"`
- `RATE_LIMIT_FAIL_OPEN`: `long, env = "RATE_LIMIT_FAIL_OPEN", default_value_t = false`
- `RPM_BUCKET_SECONDS`: `long, env = "RPM_BUCKET_SECONDS", default_value_t = 60`
- `RPM_KEY_TTL_SECONDS`: `long, env = "RPM_KEY_TTL_SECONDS", default_value_t = 120`

## Runtime reader inventory

These settings are not clap options. Defaults, parsing and supported aliases are
defined by the linked readers; the inventory does not guess types or defaults
from variable names. Request candidate persistence is a mode (`full`, `terminal`,
`none`), not a boolean. See its reader for compatibility aliases.

| Variable | Reader source |
| --- | --- |
| `ACCOUNT_SELF_CHECK_GLOBAL_CONCURRENCY` | [maintenance/runtime/account_self_check.rs](../../apps/aether-gateway/src/maintenance/runtime/account_self_check.rs) |
| `ACCOUNT_SELF_CHECK_MAX_KEYS_PER_PROVIDER` | [maintenance/runtime/account_self_check.rs](../../apps/aether-gateway/src/maintenance/runtime/account_self_check.rs) |
| `ACCOUNT_SELF_CHECK_SCAN_INTERVAL_SECONDS` | [maintenance/runtime/account_self_check.rs](../../apps/aether-gateway/src/maintenance/runtime/account_self_check.rs) |
| `AETHER_BACKUP_ENCRYPTION_KEY` | [backup/task.rs](../../apps/aether-gateway/src/backup/task.rs) |
| `AETHER_BACKUP_HISTORICAL_KEYS_JSON` | [bin/aether-backup-restore.rs](../../apps/aether-gateway/src/bin/aether-backup-restore.rs) |
| `AETHER_BARK_ALLOW_HTTP` | [bark_push.rs](../../apps/aether-gateway/src/bark_push.rs) |
| `AETHER_BARK_ALLOW_PRIVATE_TARGETS` | [bark_push.rs](../../apps/aether-gateway/src/bark_push.rs) |
| `AETHER_BASE_DIR` | [handlers/admin/system/shared/update.rs](../../apps/aether-gateway/src/handlers/admin/system/shared/update.rs) |
| `AETHER_CODEX_WS_PROBE_ACCESS_TOKEN` | [bin/aether-codex-ws-probe.rs](../../apps/aether-gateway/src/bin/aether-codex-ws-probe.rs) |
| `AETHER_CODEX_WS_PROBE_ACCOUNT_ID` | [bin/aether-codex-ws-probe.rs](../../apps/aether-gateway/src/bin/aether-codex-ws-probe.rs) |
| `AETHER_CODEX_WS_PROBE_MODEL` | [bin/aether-codex-ws-probe.rs](../../apps/aether-gateway/src/bin/aether-codex-ws-probe.rs) |
| `AETHER_CODEX_WS_PROBE_URL` | [bin/aether-codex-ws-probe.rs](../../apps/aether-gateway/src/bin/aether-codex-ws-probe.rs) |
| `AETHER_GATEWAY_ADMIN_POOL_RUNTIME_WINDOW_METRIC_KEY_LIMIT` | [handlers/admin/provider/pool/runtime/reads.rs](../../apps/aether-gateway/src/handlers/admin/provider/pool/runtime/reads.rs) |
| `AETHER_GATEWAY_AUTH_CAPACITY_CACHE_TTL_MS` | [state/app.rs](../../apps/aether-gateway/src/state/app.rs) |
| `AETHER_GATEWAY_AUTH_CONTEXT_CACHE_MAX_ENTRIES` | [control/auth/resolution.rs](../../apps/aether-gateway/src/control/auth/resolution.rs) |
| `AETHER_GATEWAY_AUTH_CONTEXT_CACHE_REFRESH_INTERVAL_SECS` | [control/auth/resolution.rs](../../apps/aether-gateway/src/control/auth/resolution.rs) |
| `AETHER_GATEWAY_AUTH_CONTEXT_NEGATIVE_CACHE_TTL_SECS` | [control/auth/resolution.rs](../../apps/aether-gateway/src/control/auth/resolution.rs) |
| `AETHER_GATEWAY_AUTH_SNAPSHOT_LOAD_GATE_LIMIT` | [state/app.rs](../../apps/aether-gateway/src/state/app.rs) |
| `AETHER_GATEWAY_BACKGROUND_DB_MAX_CONNECTIONS` | [data/config.rs](../../apps/aether-gateway/src/data/config.rs) |
| `AETHER_GATEWAY_CANDIDATE_PAGE_CACHE_STALE_TTL_MS` | [cache/candidate_page.rs](../../apps/aether-gateway/src/cache/candidate_page.rs) |
| `AETHER_GATEWAY_CANDIDATE_PAGE_CACHE_TTL_MS` | [cache/candidate_page.rs](../../apps/aether-gateway/src/cache/candidate_page.rs) |
| `AETHER_GATEWAY_CANDIDATE_PLANNING_GATE_LIMIT` | [state/app.rs](../../apps/aether-gateway/src/state/app.rs) |
| `AETHER_GATEWAY_DIRECT_H2C_ADAPTIVE_WINDOW` | [execution_runtime/transport.rs](../../apps/aether-gateway/src/execution_runtime/transport.rs) |
| `AETHER_GATEWAY_DIRECT_H2C_CLIENT_SHARDS` | [execution_runtime/transport.rs](../../apps/aether-gateway/src/execution_runtime/transport.rs) |
| `AETHER_GATEWAY_DIRECT_H2C_DRIVER_RUNTIME_THREADS` | [execution_runtime/transport.rs](../../apps/aether-gateway/src/execution_runtime/transport.rs) |
| `AETHER_GATEWAY_DIRECT_H2C_FAST_PATH` | [execution_runtime/transport.rs](../../apps/aether-gateway/src/execution_runtime/transport.rs) |
| `AETHER_GATEWAY_DIRECT_H2C_POOL_MAX_IDLE_PER_HOST` | [execution_runtime/transport.rs](../../apps/aether-gateway/src/execution_runtime/transport.rs) |
| `AETHER_GATEWAY_DIRECT_H2C_PREWARM_CONNECT_TIMEOUT_MS` | [execution_runtime/transport.rs](../../apps/aether-gateway/src/execution_runtime/transport.rs) |
| `AETHER_GATEWAY_DIRECT_H2C_PREWARM_READY` | [execution_runtime/transport.rs](../../apps/aether-gateway/src/execution_runtime/transport.rs) |
| `AETHER_GATEWAY_DIRECT_H2C_PREWARM_URLS` | [execution_runtime/transport.rs](../../apps/aether-gateway/src/execution_runtime/transport.rs) |
| `AETHER_GATEWAY_DIRECT_H2C_SENDER_SELECT_WINDOW` | [execution_runtime/transport.rs](../../apps/aether-gateway/src/execution_runtime/transport.rs) |
| `AETHER_GATEWAY_DIRECT_H2C_TARGET_STREAMS_PER_CLIENT` | [execution_runtime/transport.rs](../../apps/aether-gateway/src/execution_runtime/transport.rs) |
| `AETHER_GATEWAY_DIRECT_PASSTHROUGH_CHANNEL_CAPACITY` | [execution_runtime/stream/execution.rs](../../apps/aether-gateway/src/execution_runtime/stream/execution.rs) |
| `AETHER_GATEWAY_DIRECT_PASSTHROUGH_MODE` | [execution_runtime/stream/execution.rs](../../apps/aether-gateway/src/execution_runtime/stream/execution.rs) |
| `AETHER_GATEWAY_DIRECT_REQWEST_CACHE_MAX_ENTRIES` | [execution_runtime/transport.rs](../../apps/aether-gateway/src/execution_runtime/transport.rs) |
| `AETHER_GATEWAY_DIRECT_REQWEST_CACHE_PER_ORIGIN` | [execution_runtime/transport.rs](../../apps/aether-gateway/src/execution_runtime/transport.rs) |
| `AETHER_GATEWAY_DIRECT_REQWEST_CLIENT_SHARDS` | [execution_runtime/transport.rs](../../apps/aether-gateway/src/execution_runtime/transport.rs) |
| `AETHER_GATEWAY_DIRECT_REQWEST_H2_CLIENT_SHARDS` | [execution_runtime/transport.rs](../../apps/aether-gateway/src/execution_runtime/transport.rs) |
| `AETHER_GATEWAY_DIRECT_REQWEST_H2_TARGET_STREAMS_PER_CLIENT` | [execution_runtime/transport.rs](../../apps/aether-gateway/src/execution_runtime/transport.rs) |
| `AETHER_GATEWAY_DIRECT_REQWEST_HTTP1_TARGET_STREAMS_PER_CLIENT` | [execution_runtime/transport.rs](../../apps/aether-gateway/src/execution_runtime/transport.rs) |
| `AETHER_GATEWAY_DIRECT_REQWEST_PREWARM_SYNC_CLIENTS` | [execution_runtime/transport.rs](../../apps/aether-gateway/src/execution_runtime/transport.rs) |
| `AETHER_GATEWAY_DIRECT_REQWEST_STREAM_HTTP_MODE` | [execution_runtime/transport.rs](../../apps/aether-gateway/src/execution_runtime/transport.rs) |
| `AETHER_GATEWAY_DIRECT_REQWEST_SYNC_WARM_CLIENTS` | [execution_runtime/transport.rs](../../apps/aether-gateway/src/execution_runtime/transport.rs) |
| `AETHER_GATEWAY_ERROR_DETAIL_LOGGING` | [error.rs](../../apps/aether-gateway/src/error.rs) |
| `AETHER_GATEWAY_EXTERNAL_MODELS_URL` | [handlers/admin/model/external_cache.rs](../../apps/aether-gateway/src/handlers/admin/model/external_cache.rs) |
| `AETHER_GATEWAY_INSTANCE_ID` | [main.rs](../../apps/aether-gateway/src/main.rs), [tunnel/mod.rs](../../apps/aether-gateway/src/tunnel/mod.rs) |
| `AETHER_GATEWAY_INTERNAL_GATE_QUEUE_BUDGET_MS` | [state/app.rs](../../apps/aether-gateway/src/state/app.rs) |
| `AETHER_GATEWAY_LOCAL_EXECUTION_PLANNING_TIMEOUT_MS` | [state/app.rs](../../apps/aether-gateway/src/state/app.rs) |
| `AETHER_GATEWAY_OPENAI_CHAT_STREAM_TARGET_SELECT_WINDOW` | [ai_serving/planner/standard/openai/chat/plans/stream.rs](../../apps/aether-gateway/src/ai_serving/planner/standard/openai/chat/plans/stream.rs) |
| `AETHER_GATEWAY_POOL_SCORE_FAILURE_FEEDBACK_MIN_INTERVAL_SECS` | [orchestration/effects.rs](../../apps/aether-gateway/src/orchestration/effects.rs) |
| `AETHER_GATEWAY_POOL_SCORE_SUCCESS_FEEDBACK_MIN_INTERVAL_SECS` | [orchestration/effects.rs](../../apps/aether-gateway/src/orchestration/effects.rs) |
| `AETHER_GATEWAY_PROVIDER_KEY_ADAPTIVE_SUCCESS_PERSIST_MIN_INTERVAL_SECS` | [orchestration/effects.rs](../../apps/aether-gateway/src/orchestration/effects.rs) |
| `AETHER_GATEWAY_PROVIDER_KEY_HEALTH_SUCCESS_PERSIST_MIN_INTERVAL_SECS` | [orchestration/effects.rs](../../apps/aether-gateway/src/orchestration/effects.rs) |
| `AETHER_GATEWAY_PROVIDER_POOL_IN_FLIGHT_ACQUIRE_TIMEOUT_MS` | [provider_pool_demand.rs](../../apps/aether-gateway/src/provider_pool_demand.rs) |
| `AETHER_GATEWAY_PROVIDER_POOL_IN_FLIGHT_MODE` | [provider_pool_demand.rs](../../apps/aether-gateway/src/provider_pool_demand.rs) |
| `AETHER_GATEWAY_REQUEST_BODY_BUFFER_BUDGET_MB` | [state/app.rs](../../apps/aether-gateway/src/state/app.rs) |
| `AETHER_GATEWAY_REQUEST_BODY_READ_TIMEOUT_MS` | [state/app.rs](../../apps/aether-gateway/src/state/app.rs) |
| `AETHER_GATEWAY_REQUEST_CANDIDATE_DB_BATCH_SIZE` | [request_candidate_queue.rs](../../apps/aether-gateway/src/request_candidate_queue.rs) |
| `AETHER_GATEWAY_REQUEST_CANDIDATE_DB_WRITE_CONCURRENCY_LIMIT` | [request_candidate_queue.rs](../../apps/aether-gateway/src/request_candidate_queue.rs) |
| `AETHER_GATEWAY_REQUEST_CANDIDATE_PERSISTENCE` | [request_candidate_runtime.rs](../../apps/aether-gateway/src/request_candidate_runtime.rs) |
| `AETHER_GATEWAY_REQUEST_CANDIDATE_QUEUE_BATCH_SIZE` | [request_candidate_queue.rs](../../apps/aether-gateway/src/request_candidate_queue.rs) |
| `AETHER_GATEWAY_REQUEST_CANDIDATE_QUEUE_CAPACITY` | [request_candidate_queue.rs](../../apps/aether-gateway/src/request_candidate_queue.rs) |
| `AETHER_GATEWAY_REQUEST_CANDIDATE_QUEUE_FLUSH_INTERVAL_MS` | [request_candidate_queue.rs](../../apps/aether-gateway/src/request_candidate_queue.rs) |
| `AETHER_GATEWAY_REQUEST_CANDIDATE_QUEUE_FULL` | [request_candidate_queue.rs](../../apps/aether-gateway/src/request_candidate_queue.rs) |
| `AETHER_GATEWAY_REQUEST_CANDIDATE_QUEUE_RUNTIME_THREADS` | [request_candidate_queue.rs](../../apps/aether-gateway/src/request_candidate_queue.rs) |
| `AETHER_GATEWAY_REQUEST_CANDIDATE_QUEUE_WORKERS` | [request_candidate_queue.rs](../../apps/aether-gateway/src/request_candidate_queue.rs) |
| `AETHER_GATEWAY_REQUEST_CANDIDATE_SEED_WRITE_TIMEOUT_MS` | [request_candidate_runtime.rs](../../apps/aether-gateway/src/request_candidate_runtime.rs) |
| `AETHER_GATEWAY_REQUEST_CANDIDATE_WRITE_MODE` | [request_candidate_queue.rs](../../apps/aether-gateway/src/request_candidate_queue.rs) |
| `AETHER_GATEWAY_SECURITY_CACHE_TTL_MS` | [state/runtime/security.rs](../../apps/aether-gateway/src/state/runtime/security.rs) |
| `AETHER_GATEWAY_STAGE_METRICS_ENABLED` | [stage_metrics.rs](../../apps/aether-gateway/src/stage_metrics.rs) |
| `AETHER_GATEWAY_STAGE_TRACE_MODE` | [stage_metrics.rs](../../apps/aether-gateway/src/stage_metrics.rs) |
| `AETHER_GATEWAY_STAGE_TRACE_SAMPLE_RATE` | [stage_metrics.rs](../../apps/aether-gateway/src/stage_metrics.rs) |
| `AETHER_GATEWAY_STAGE_TRACE_SLOW_MS` | [stage_metrics.rs](../../apps/aether-gateway/src/stage_metrics.rs) |
| `AETHER_GATEWAY_STREAM_CAPTURE_MEMORY_BUDGET_BYTES` | [execution_runtime/stream/capture_budget.rs](../../apps/aether-gateway/src/execution_runtime/stream/capture_budget.rs) |
| `AETHER_GATEWAY_TRUSTED_INGRESS_CIDRS` | [headers.rs](../../apps/aether-gateway/src/headers.rs) |
| `AETHER_GATEWAY_UPSTREAM_EXECUTION_GATE_HOLD_STREAM_RESPONSE` | [executor/candidate_loop.rs](../../apps/aether-gateway/src/executor/candidate_loop.rs) |
| `AETHER_GATEWAY_UPSTREAM_EXECUTION_GATE_LIMIT` | [state/app.rs](../../apps/aether-gateway/src/state/app.rs) |
| `AETHER_GATEWAY_UPSTREAM_EXECUTION_GATE_STREAM_HOLD_MODE` | [executor/candidate_loop.rs](../../apps/aether-gateway/src/executor/candidate_loop.rs) |
| `AETHER_GATEWAY_UPSTREAM_POOL_IDLE_TIMEOUT_MS` | [execution_runtime/transport.rs](../../apps/aether-gateway/src/execution_runtime/transport.rs) |
| `AETHER_GATEWAY_UPSTREAM_POOL_MAX_IDLE_PER_HOST` | [execution_runtime/transport.rs](../../apps/aether-gateway/src/execution_runtime/transport.rs) |
| `AETHER_GATEWAY_UPSTREAM_STREAM_IDLE_TIMEOUT_MS` | [execution_runtime/stream_read_timeout.rs](../../apps/aether-gateway/src/execution_runtime/stream_read_timeout.rs) |
| `AETHER_GATEWAY_UPSTREAM_TARGET_GATE_LIMIT` | [state/app.rs](../../apps/aether-gateway/src/state/app.rs) |
| `AETHER_GATEWAY_UPSTREAM_TARGET_GATE_METRIC_LIMIT` | [upstream_admission.rs](../../apps/aether-gateway/src/upstream_admission.rs) |
| `AETHER_GATEWAY_UPSTREAM_TARGET_GATE_QUEUE_BUDGET_MS` | [upstream_admission.rs](../../apps/aether-gateway/src/upstream_admission.rs) |
| `AETHER_GATEWAY_USAGE_COUNTER_DELTA_CLEANUP_BATCH_SIZE` | [maintenance/runtime/usage_counter_flush.rs](../../apps/aether-gateway/src/maintenance/runtime/usage_counter_flush.rs) |
| `AETHER_GATEWAY_USAGE_COUNTER_DELTA_CLEANUP_INTERVAL_MS` | [maintenance/runtime/usage_counter_flush.rs](../../apps/aether-gateway/src/maintenance/runtime/usage_counter_flush.rs) |
| `AETHER_GATEWAY_USAGE_COUNTER_DELTA_RETENTION_SECS` | [maintenance/runtime/usage_counter_flush.rs](../../apps/aether-gateway/src/maintenance/runtime/usage_counter_flush.rs) |
| `AETHER_GATEWAY_USAGE_COUNTER_FLUSH_BATCH_SIZE` | [maintenance/runtime/usage_counter_flush.rs](../../apps/aether-gateway/src/maintenance/runtime/usage_counter_flush.rs) |
| `AETHER_GATEWAY_USAGE_COUNTER_FLUSH_CATCH_UP_BURST_LIMIT` | [maintenance/runtime/usage_counter_flush.rs](../../apps/aether-gateway/src/maintenance/runtime/usage_counter_flush.rs) |
| `AETHER_GATEWAY_USAGE_COUNTER_FLUSH_INTERVAL_MS` | [maintenance/runtime/usage_counter_flush.rs](../../apps/aether-gateway/src/maintenance/runtime/usage_counter_flush.rs) |
| `AETHER_INTERNAL_GATEWAY_AUTH_SECRET` | [internal_gateway_auth.rs](../../apps/aether-gateway/src/internal_gateway_auth.rs) |
| `AETHER_MAX_INTERNAL_BUFFERED_BODY_MB` | [headers.rs](../../apps/aether-gateway/src/headers.rs) |
| `AETHER_MAX_REDACTED_SYNC_RESPONSE_BODY_MB` | [headers.rs](../../apps/aether-gateway/src/headers.rs) |
| `AETHER_MAX_REQUEST_BODY_MB` | [headers.rs](../../apps/aether-gateway/src/headers.rs) |
| `AETHER_OPENAI_WS_PROBE_API_KEY` | [bin/aether-openai-responses-ws-probe.rs](../../apps/aether-gateway/src/bin/aether-openai-responses-ws-probe.rs) |
| `AETHER_OPENAI_WS_PROBE_MODEL` | [bin/aether-openai-responses-ws-probe.rs](../../apps/aether-gateway/src/bin/aether-openai-responses-ws-probe.rs) |
| `AETHER_OPENAI_WS_PROBE_URL` | [bin/aether-openai-responses-ws-probe.rs](../../apps/aether-gateway/src/bin/aether-openai-responses-ws-probe.rs) |
| `AETHER_PUBLIC_BASE_URL` | [handlers/public/support/auth_cookie_policy.rs](../../apps/aether-gateway/src/handlers/public/support/auth_cookie_policy.rs), [handlers/public/support/install.rs](../../apps/aether-gateway/src/handlers/public/support/install.rs), [handlers/public/support/payment/epay.rs](../../apps/aether-gateway/src/handlers/public/support/payment/epay.rs) |
| `AETHER_TRUSTED_PROXY_CIDRS` | [headers.rs](../../apps/aether-gateway/src/headers.rs) |
| `AETHER_TUNNEL_ATTACHMENT_TTL_SECS` | [tunnel/mod.rs](../../apps/aether-gateway/src/tunnel/mod.rs) |
| `AETHER_TUNNEL_BASE_URL` | [execution_runtime/transport.rs](../../apps/aether-gateway/src/execution_runtime/transport.rs) |
| `AETHER_TUNNEL_DRAIN_DEADLINE_MS` | [tunnel/embedded/hub.rs](../../apps/aether-gateway/src/tunnel/embedded/hub.rs) |
| `AETHER_TUNNEL_NODE_STATUS_QUEUE_CAPACITY` | [tunnel/embedded/hub.rs](../../apps/aether-gateway/src/tunnel/embedded/hub.rs) |
| `AETHER_TUNNEL_RELAY_ALLOW_PRIVATE_TARGETS` | [tunnel/mod.rs](../../apps/aether-gateway/src/tunnel/mod.rs) |
| `AETHER_TUNNEL_RELAY_AUTH_SECRET` | [execution_runtime/transport.rs](../../apps/aether-gateway/src/execution_runtime/transport.rs), [tunnel/mod.rs](../../apps/aether-gateway/src/tunnel/mod.rs) |
| `AETHER_TUNNEL_RELAY_BASE_URL` | [main.rs](../../apps/aether-gateway/src/main.rs), [tunnel/mod.rs](../../apps/aether-gateway/src/tunnel/mod.rs) |
| `AETHER_TUNNEL_RELAY_MAX_BODY_MB` | [tunnel/mod.rs](../../apps/aether-gateway/src/tunnel/mod.rs) |
| `AETHER_TUNNEL_RELAY_PRIVATE_HOST_ALLOWLIST` | [tunnel/mod.rs](../../apps/aether-gateway/src/tunnel/mod.rs) |
| `AETHER_TUNNEL_RELAY_SPOOL_BUDGET_MB` | [tunnel/mod.rs](../../apps/aether-gateway/src/tunnel/mod.rs) |
| `AETHER_TUNNEL_STREAM_INITIAL_WINDOW_BYTES` | [tunnel/embedded/hub.rs](../../apps/aether-gateway/src/tunnel/embedded/hub.rs) |
| `AETHER_UPDATE_DOWNLOAD_IDLE_TIMEOUT_SECS` | [handlers/admin/system/shared/update.rs](../../apps/aether-gateway/src/handlers/admin/system/shared/update.rs) |
| `AETHER_UPDATE_DOWNLOAD_TIMEOUT_SECS` | [handlers/admin/system/shared/update.rs](../../apps/aether-gateway/src/handlers/admin/system/shared/update.rs) |
| `AETHER_UPDATE_STRATEGY` | [handlers/admin/system/shared/update.rs](../../apps/aether-gateway/src/handlers/admin/system/shared/update.rs) |
| `AETHER_VSCODEX_ENABLED` | [handlers/public/support/user_me_vscodex.rs](../../apps/aether-gateway/src/handlers/public/support/user_me_vscodex.rs) |
| `AETHER_VSCODEX_INTERNAL_TOKEN` | [handlers/public/support/user_me_vscodex.rs](../../apps/aether-gateway/src/handlers/public/support/user_me_vscodex.rs) |
| `AETHER_VSCODEX_INTERNAL_URL` | [handlers/public/support/user_me_vscodex.rs](../../apps/aether-gateway/src/handlers/public/support/user_me_vscodex.rs) |
| `AETHER_WINDSURF_FORCE_GPT_NATIVE_DIALECT` | [execution_runtime/windsurf.rs](../../apps/aether-gateway/src/execution_runtime/windsurf.rs) |
| `AETHER_WINDSURF_NATIVE_TOOL_BRIDGE` | [execution_runtime/windsurf.rs](../../apps/aether-gateway/src/execution_runtime/windsurf.rs) |
| `AETHER_WINDSURF_NATIVE_TOOL_BRIDGE_OFF` | [execution_runtime/windsurf.rs](../../apps/aether-gateway/src/execution_runtime/windsurf.rs) |
| `APP_TIMEZONE` | [app_timezone.rs](../../apps/aether-gateway/src/app_timezone.rs), [backup/schedule.rs](../../apps/aether-gateway/src/backup/schedule.rs), [maintenance/runtime/schedule.rs](../../apps/aether-gateway/src/maintenance/runtime/schedule.rs), [plan_usage_policy.rs](../../apps/aether-gateway/src/plan_usage_policy.rs) |
| `AUTH_REFRESH_COOKIE_NAME` | [handlers/public/support/auth_helpers.rs](../../apps/aether-gateway/src/handlers/public/support/auth_helpers.rs) |
| `AUTH_REFRESH_COOKIE_SAMESITE` | [handlers/public/support/auth_helpers.rs](../../apps/aether-gateway/src/handlers/public/support/auth_helpers.rs) |
| `AUTH_REFRESH_COOKIE_SECURE` | [handlers/public/support/auth_cookie_policy.rs](../../apps/aether-gateway/src/handlers/public/support/auth_cookie_policy.rs), [handlers/public/support/auth_helpers.rs](../../apps/aether-gateway/src/handlers/public/support/auth_helpers.rs) |
| `BATCH_BALANCE_CONCURRENCY` | [handlers/admin/provider/ops/providers/balance_cache.rs](../../apps/aether-gateway/src/handlers/admin/provider/ops/providers/balance_cache.rs) |
| `CODEIUM_API_URL` | [execution_runtime/windsurf.rs](../../apps/aether-gateway/src/execution_runtime/windsurf.rs) |
| `DATABASE_URL` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `ENCRYPTION_KEY` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `HOME` | [execution_runtime/windsurf.rs](../../apps/aether-gateway/src/execution_runtime/windsurf.rs) |
| `HOSTNAME` | [main.rs](../../apps/aether-gateway/src/main.rs), [tunnel/mod.rs](../../apps/aether-gateway/src/tunnel/mod.rs) |
| `JWT_EXPIRATION_HOURS` | [handlers/public/support/auth_helpers.rs](../../apps/aether-gateway/src/handlers/public/support/auth_helpers.rs) |
| `JWT_SECRET_KEY` | [local_auth_token.rs](../../apps/aether-gateway/src/local_auth_token.rs), [state/testing.rs](../../apps/aether-gateway/src/state/testing.rs) |
| `LDAP_AVAILABLE` | [handlers/public/support/auth_helpers.rs](../../apps/aether-gateway/src/handlers/public/support/auth_helpers.rs), [handlers/public/support/auth_ldap.rs](../../apps/aether-gateway/src/handlers/public/support/auth_ldap.rs) |
| `MANAGEMENT_TOKEN_MAX_PER_USER` | [handlers/public/support/user_me_management_tokens.rs](../../apps/aether-gateway/src/handlers/public/support/user_me_management_tokens.rs) |
| `MODEL_FETCH_STARTUP_DELAY_SECONDS` | [model_fetch/tests.rs](../../apps/aether-gateway/src/model_fetch/tests.rs) |
| `MODEL_FETCH_STARTUP_ENABLED` | [model_fetch/tests.rs](../../apps/aether-gateway/src/model_fetch/tests.rs) |
| `NOTIFICATION_EMAIL_AVAILABLE` | [handlers/admin/system/shared/modules.rs](../../apps/aether-gateway/src/handlers/admin/system/shared/modules.rs) |
| `OAUTH_AVAILABLE` | [oauth/identity_repo.rs](../../apps/aether-gateway/src/oauth/identity_repo.rs) |
| `PAYMENT_CALLBACK_SECRET` | [handlers/public/support/payment/shared.rs](../../apps/aether-gateway/src/handlers/public/support/payment/shared.rs) |
| `POOL_QUOTA_PROBE_GLOBAL_CONCURRENCY` | [maintenance/runtime/pool_quota_probe.rs](../../apps/aether-gateway/src/maintenance/runtime/pool_quota_probe.rs) |
| `POOL_QUOTA_PROBE_MAX_KEYS_PER_PROVIDER` | [maintenance/runtime/pool_quota_probe.rs](../../apps/aether-gateway/src/maintenance/runtime/pool_quota_probe.rs) |
| `POOL_QUOTA_PROBE_SCAN_INTERVAL_SECONDS` | [maintenance/runtime/pool_quota_probe.rs](../../apps/aether-gateway/src/maintenance/runtime/pool_quota_probe.rs) |
| `POOL_SCORE_REBUILD_INTERVAL_SECONDS` | [maintenance/runtime/pool_score_rebuild.rs](../../apps/aether-gateway/src/maintenance/runtime/pool_score_rebuild.rs) |
| `POOL_SCORE_REBUILD_MAX_UPSERTS_PER_TICK` | [maintenance/runtime/pool_score_rebuild.rs](../../apps/aether-gateway/src/maintenance/runtime/pool_score_rebuild.rs) |
| `PUBLIC_BASE_URL` | [handlers/public/support/auth_cookie_policy.rs](../../apps/aether-gateway/src/handlers/public/support/auth_cookie_policy.rs), [handlers/public/support/install.rs](../../apps/aether-gateway/src/handlers/public/support/install.rs), [handlers/public/support/payment/epay.rs](../../apps/aether-gateway/src/handlers/public/support/payment/epay.rs) |
| `REDIS_URL` | [main.rs](../../apps/aether-gateway/src/main.rs) |
| `USERPROFILE` | [execution_runtime/windsurf.rs](../../apps/aether-gateway/src/execution_runtime/windsurf.rs) |
| `VERIFICATION_CODE_EXPIRE_MINUTES` | [handlers/public/support/auth_helpers.rs](../../apps/aether-gateway/src/handlers/public/support/auth_helpers.rs) |
| `VERIFICATION_SEND_COOLDOWN` | [handlers/public/support/auth_helpers.rs](../../apps/aether-gateway/src/handlers/public/support/auth_helpers.rs) |
| `WINDSURFAPI_FORCE_GPT_NATIVE_DIALECT` | [execution_runtime/windsurf.rs](../../apps/aether-gateway/src/execution_runtime/windsurf.rs) |
| `WINDSURFAPI_NATIVE_TOOL_BRIDGE` | [execution_runtime/windsurf.rs](../../apps/aether-gateway/src/execution_runtime/windsurf.rs) |
| `WINDSURFAPI_NATIVE_TOOL_BRIDGE_OFF` | [execution_runtime/windsurf.rs](../../apps/aether-gateway/src/execution_runtime/windsurf.rs) |
