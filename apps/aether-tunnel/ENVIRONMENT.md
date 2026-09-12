# Tunnel 环境变量参考

本文件由实际 `Config::command()` 的 clap 元数据生成。不要手工编辑；更新命令：

```bash
cargo test -p aether-tunnel --bin aether-tunnel config::env_reference::regenerate_tunnel_env_reference -- --ignored --exact
```

校验命令：`cargo test -p aether-tunnel --bin aether-tunnel config::env_reference`。
Rust CI 的 `Test (Workspace Rest)` 会运行同一校验；环境变量名、CLI 名、默认值、说明或枚举值变化都会要求重新生成。

## 读取规则

- 默认值列只导出 clap 声明的默认值，不导出当前环境变量值或实际凭据。CLI 覆盖环境变量，环境变量覆盖 TOML。
- `必填` 表示 clap 无默认值；直接运行需提供值，使用 TOML 时由配置加载器提供。多服务器的 URL、Token 和可选节点覆盖放在 `[[servers]]` 中。
- `未设置` 表示 clap 的 `Option` 为 `None`，不等于零或一律禁用。公网 IP、地区、并发及连接池有启动时探测或推导；详见 [README](README.md#配置) 和 `src/app.rs` / `Config::resolve_tunnel_pool_sizing`。
- 省略 `diagnostics_bind` 不启动诊断监听；省略 `distributed_stream_limit` 不启用跨实例 admission，启用时必须同时配置 Redis URL。省略两个出口 proxy URL 时对应流量直连。
- 此表覆盖全部 clap 环境变量，包括标记为隐藏的兼容参数。兼容参数只接受输入，不生效；不能用于限制资源。
- 秒、毫秒和字节以单位列为准，不要根据环境变量是否带 `_SECS` 猜测。clap 不会为拼错的名称自动创建别名，未知环境变量可能被忽略。

## 启动和安装脚本变量

以下变量不属于 clap 参数，分别由启动入口和安装脚本读取，因此不在生成表内：

| 环境变量 | 默认或省略行为 | 读取方 |
| --- | --- | --- |
| `AETHER_TUNNEL_CONFIG` | `aether-tunnel.toml`（路径） | `src/main.rs`；安装脚本也接受配置路径 |
| `AETHER_TUNNEL_RELEASE_TAG` | 自动选择最新 tunnel tag | `install.sh` / `install.ps1` |
| `AETHER_TUNNEL_INSTALL_DIR` | 按系统选择安装目录 | `install.sh` / `install.ps1` |

## clap 参数

| 环境变量 | CLI 参数 | clap 默认值 | 单位 / 类型 | 说明 |
| --- | --- | --- | --- | --- |
| `AETHER_TUNNEL_AETHER_CONNECT_TIMEOUT` | `--aether-connect-timeout-secs` | `10` | 秒 | Aether API connect timeout in seconds |
| `AETHER_TUNNEL_AETHER_HTTP2` | `--aether-http2` | `true` | 布尔值 | Enable HTTP/2 when talking to Aether API 可选值：`true`, `false`。 |
| `AETHER_TUNNEL_AETHER_OUTBOUND_PROXY_URL` | `--aether-outbound-proxy-url` | 未设置 | URL | Optional egress proxy used for Aether API registration and WebSocket tunnel reconnects. Supported schemes: http, socks5, socks5h |
| `AETHER_TUNNEL_AETHER_POOL_IDLE_TIMEOUT` | `--aether-pool-idle-timeout-secs` | `90` | 秒 | Aether API idle timeout in seconds |
| `AETHER_TUNNEL_AETHER_POOL_MAX_IDLE_PER_HOST` | `--aether-pool-max-idle-per-host` | `8` | 连接数 | Aether API max idle connections per host |
| `AETHER_TUNNEL_AETHER_REQUEST_TIMEOUT` | `--aether-request-timeout-secs` | `10` | 秒 | Aether API request timeout in seconds |
| `AETHER_TUNNEL_AETHER_RETRY_BASE_DELAY_MS` | `--aether-retry-base-delay-ms` | `200` | 毫秒 | Aether API retry base delay in milliseconds |
| `AETHER_TUNNEL_AETHER_RETRY_MAX_ATTEMPTS` | `--aether-retry-max-attempts` | `3` | 尝试次数（含首次） | Aether API retry attempts (including initial) |
| `AETHER_TUNNEL_AETHER_RETRY_MAX_DELAY_MS` | `--aether-retry-max-delay-ms` | `2000` | 毫秒 | Aether API retry max delay in milliseconds |
| `AETHER_TUNNEL_AETHER_TCP_KEEPALIVE` | `--aether-tcp-keepalive-secs` | `60` | 秒 | Aether API TCP keepalive in seconds (0 disables) |
| `AETHER_TUNNEL_AETHER_TCP_NODELAY` | `--aether-tcp-nodelay` | `true` | 布尔值 | Aether API TCP_NODELAY 可选值：`true`, `false`。 |
| `AETHER_TUNNEL_AETHER_URL` | `--aether-url` | 必填 | URL | Aether server URL (e.g. https://aether.example.com) |
| `AETHER_TUNNEL_ALLOWED_PORTS` | `--allowed-ports` | `80,443,8080,8443` | 端口号（逗号分隔） | Allowed destination ports (default: 80,443,8080,8443) |
| `AETHER_TUNNEL_ALLOW_PRIVATE_TARGETS` | `--allow-private-targets` | `false` | 布尔值 | Allow private/reserved upstream IP targets. Disabled by default; enable explicitly only for deployments that require access to private services 可选值：`true`, `false`。 |
| `AETHER_TUNNEL_CONNECTIONS` | `--tunnel-connections` | 未设置 | 连接数 | Minimum number of parallel WebSocket tunnel connections per server. If omitted, a device-aware redundant value is auto-detected at startup |
| `AETHER_TUNNEL_CONNECTIONS_MAX` | `--tunnel-connections-max` | 未设置 | 连接数 | Maximum number of WebSocket tunnel connections per server. When larger than `tunnel_connections`, the tunnel may autoscale up to this limit |
| `AETHER_TUNNEL_CONNECT_TIMEOUT_MS` | `--tunnel-connect-timeout-ms` | `3000` | 毫秒 | WebSocket tunnel TCP connect timeout in milliseconds |
| `AETHER_TUNNEL_DIAGNOSTICS_BIND` | `--diagnostics-bind` | 未设置 | IP:端口 | Optional local diagnostics listener for /health, /metrics, and /stats. Bind only to loopback addresses, for example 127.0.0.1:9311 |
| `AETHER_TUNNEL_DISTRIBUTED_STREAM_COMMAND_TIMEOUT_MS` | `--distributed-stream-command-timeout-ms` | `1000` | 毫秒 | Command timeout in milliseconds for distributed stream admission Redis calls |
| `AETHER_TUNNEL_DISTRIBUTED_STREAM_LEASE_TTL_MS` | `--distributed-stream-lease-ttl-ms` | `30000` | 毫秒 | Lease TTL in milliseconds for distributed stream admission permits |
| `AETHER_TUNNEL_DISTRIBUTED_STREAM_LIMIT` | `--distributed-stream-limit` | 未设置 | stream 数 | Maximum in-flight tunneled streams admitted across all tunnel instances |
| `AETHER_TUNNEL_DISTRIBUTED_STREAM_REDIS_KEY_PREFIX` | `--distributed-stream-redis-key-prefix` | 未设置 | 字符串 | Optional key prefix for cross-instance stream admission state |
| `AETHER_TUNNEL_DISTRIBUTED_STREAM_REDIS_URL` | `--distributed-stream-redis-url` | 未设置 | URL | Redis URL used for cross-instance stream admission |
| `AETHER_TUNNEL_DISTRIBUTED_STREAM_RENEW_INTERVAL_MS` | `--distributed-stream-renew-interval-ms` | `10000` | 毫秒 | Renew interval in milliseconds for distributed stream admission permits |
| `AETHER_TUNNEL_DNS_CACHE_CAPACITY` | `--dns-cache-capacity` | `1024` | 条目数 | DNS cache capacity (entries) |
| `AETHER_TUNNEL_DNS_CACHE_TTL` | `--dns-cache-ttl-secs` | `60` | 秒 | DNS cache TTL in seconds |
| `AETHER_TUNNEL_DRAIN_DEADLINE_MS` | `--tunnel-drain-deadline-ms` | `30000` | 毫秒 | Deadline for graceful tunnel drain after GOAWAY |
| `AETHER_TUNNEL_EMIT_PROXY_TIMING_HEADER` | `--emit-proxy-timing-header` | `true` | 布尔值 | Emit detailed x-proxy-timing headers on tunneled upstream responses 可选值：`true`, `false`。 |
| `AETHER_TUNNEL_ENCRYPTION_KEY` | `--tunnel-encryption-key` | 未设置 | base64（32 字节 PSK） | Base64-encoded 32-byte PSK used when tunnel_security=non_tls_required |
| `AETHER_TUNNEL_HEARTBEAT_INTERVAL` | `--heartbeat-interval` | `5` | 秒 | Heartbeat interval in seconds |
| `AETHER_TUNNEL_IPV4_ONLY` | `--tunnel-ipv4-only` | `false` | 布尔值 | Force direct WebSocket tunnel TCP connects, or Aether outbound proxy endpoint connects, to IPv4 addresses only 可选值：`true`, `false`。 |
| `AETHER_TUNNEL_IPV6_ONLY` | `--tunnel-ipv6-only` | `false` | 布尔值 | Force direct WebSocket tunnel TCP connects, or Aether outbound proxy endpoint connects, to IPv6 addresses only 可选值：`true`, `false`。 |
| `AETHER_TUNNEL_LOG_DESTINATION` | `--log-destination` | `both` | 字符串 | Log destination (stdout, file, both) 可选值：`stdout`, `file`, `both`。 |
| `AETHER_TUNNEL_LOG_DIR` | `--log-dir` | `logs` | 路径 | Log directory when file logging is enabled |
| `AETHER_TUNNEL_LOG_LEVEL` | `--log-level` | `info` | 字符串 | Log level (trace, debug, info, warn, error) |
| `AETHER_TUNNEL_LOG_MAX_FILES` | `--log-max-files` | `30` | 文件数 | Maximum number of retained rolled log files |
| `AETHER_TUNNEL_LOG_RETENTION_DAYS` | `--log-retention-days` | `7` | 天 | Log file retention days for file logging |
| `AETHER_TUNNEL_LOG_ROTATION` | `--log-rotation` | `daily` | 字符串 | Log rotation schedule for file logging 可选值：`hourly`, `daily`。 |
| `AETHER_TUNNEL_MANAGEMENT_TOKEN` | `--management-token` | 必填 | 字符串 | Management Token for Aether admin API (ae_xxx) |
| `AETHER_TUNNEL_MAX_CONCURRENT_CONNECTIONS` | `--max-concurrent-connections` | 未设置 | 连接数 | Maximum concurrent TCP connections (defaults to hardware estimate) |
| `AETHER_TUNNEL_MAX_IN_FLIGHT_STREAMS` | `--max-in-flight-streams` | 未设置 | stream 数 | Maximum in-flight tunneled streams accepted by this tunnel instance |
| `AETHER_TUNNEL_MAX_STREAMS` | `--tunnel-max-streams` | 未设置 | stream 数 | Maximum concurrent streams over tunnel (auto-detected from hardware if omitted) |
| `AETHER_TUNNEL_NODE_NAME` | `--node-name` | 必填 | 字符串 | Human-readable node name |
| `AETHER_TUNNEL_NODE_REGION` | `--node-region` | 未设置 | 字符串 | Region label (e.g. ap-northeast-1) |
| `AETHER_TUNNEL_PING_INTERVAL_MS` | `--tunnel-ping-interval-ms` | `10000` | 毫秒 | WebSocket tunnel ping interval in milliseconds |
| `AETHER_TUNNEL_PROFILE` | `--tunnel-profile` | `standard` | 字符串 | Tunnel connection pool profile used when connection counts are not explicit 可选值：`lite`, `standard`, `throughput`。 |
| `AETHER_TUNNEL_PUBLIC_IP` | `--public-ip` | 未设置 | IP 地址 | Public IP address of this node (auto-detected if omitted) |
| `AETHER_TUNNEL_RECONNECT_BASE_MS` | `--tunnel-reconnect-base-ms` | `50` | 毫秒 | Tunnel reconnect base delay in milliseconds (used by exponential backoff) |
| `AETHER_TUNNEL_RECONNECT_MAX_MS` | `--tunnel-reconnect-max-ms` | `250` | 毫秒 | Tunnel reconnect max delay in milliseconds (cap for exponential backoff) |
| `AETHER_TUNNEL_REDIRECT_REPLAY_BUDGET_BYTES` | `--redirect-replay-budget-bytes` | 未设置 | 字节 | **隐藏兼容参数；输入被忽略。** Accepted only so older launch commands and environments keep working. Redirect request bodies are always replayed without a cumulative size limit |
| `AETHER_TUNNEL_REMOTE_UPGRADE_ENABLED` | `--remote-upgrade-enabled` | `false` | 布尔值 | Allow heartbeat ACKs to trigger a self-upgrade.  Remote upgrades are disabled by default until release artifact signature verification is configured for this installation. 可选值：`true`, `false`。 |
| `AETHER_TUNNEL_SCALE_CHECK_INTERVAL_MS` | `--tunnel-scale-check-interval-ms` | `1000` | 毫秒 | Autoscale evaluation interval for the tunnel pool |
| `AETHER_TUNNEL_SCALE_DOWN_GRACE_SECS` | `--tunnel-scale-down-grace-secs` | `15` | 秒 | Low-load grace window before a secondary tunnel is drained |
| `AETHER_TUNNEL_SCALE_DOWN_THRESHOLD_PERCENT` | `--tunnel-scale-down-threshold-percent` | `35` | % | Per-tunnel occupancy percentage that allows scale-down after the grace window |
| `AETHER_TUNNEL_SCALE_UP_THRESHOLD_PERCENT` | `--tunnel-scale-up-threshold-percent` | `50` | % | Per-tunnel occupancy percentage that triggers scale-up |
| `AETHER_TUNNEL_SECURITY` | `--tunnel-security` | `off` | 字符串 | Application-layer tunnel security mode |
| `AETHER_TUNNEL_STALE_TIMEOUT_MS` | `--tunnel-stale-timeout-ms` | `30000` | 毫秒 | Tunnel connection staleness timeout in milliseconds |
| `AETHER_TUNNEL_STREAM_INITIAL_WINDOW_BYTES` | `--tunnel-stream-initial-window-bytes` | `4194304` | 字节 | Initial per-stream flow-control window advertised by this tunnel |
| `AETHER_TUNNEL_TCP_KEEPALIVE` | `--tunnel-tcp-keepalive-secs` | `30` | 秒 | WebSocket tunnel TCP keepalive in seconds (0 disables) |
| `AETHER_TUNNEL_TCP_NODELAY` | `--tunnel-tcp-nodelay` | `true` | 布尔值 | WebSocket tunnel TCP_NODELAY 可选值：`true`, `false`。 |
| `AETHER_TUNNEL_UPSTREAM_CLIENT_POOL_CAPACITY` | `--upstream-client-pool-capacity` | `256` | client 数 | Maximum number of keyed upstream HTTP clients retained by the tunnel |
| `AETHER_TUNNEL_UPSTREAM_CONNECT_TIMEOUT` | `--upstream-connect-timeout-secs` | `30` | 秒 | Upstream HTTP client connect timeout in seconds |
| `AETHER_TUNNEL_UPSTREAM_POOL_IDLE_TIMEOUT` | `--upstream-pool-idle-timeout-secs` | `300` | 秒 | Upstream HTTP client idle timeout in seconds |
| `AETHER_TUNNEL_UPSTREAM_POOL_MAX_IDLE_PER_HOST` | `--upstream-pool-max-idle-per-host` | `64` | 连接数 | Upstream HTTP client max idle connections per host |
| `AETHER_TUNNEL_UPSTREAM_PROXY_REMOTE_DNS` | `--upstream-proxy-remote-dns` | `false` | 布尔值 | Trust an HTTP or SOCKS5h upstream proxy to resolve hostnames and enforce destination IP access controls 可选值：`true`, `false`。 |
| `AETHER_TUNNEL_UPSTREAM_PROXY_URL` | `--upstream-proxy-url` | 未设置 | URL | Optional egress proxy used only for provider upstream requests. Supported schemes: http, socks5, socks5h |
| `AETHER_TUNNEL_UPSTREAM_TCP_KEEPALIVE` | `--upstream-tcp-keepalive-secs` | `60` | 秒 | Upstream TCP keepalive in seconds (0 disables) |
| `AETHER_TUNNEL_UPSTREAM_TCP_NODELAY` | `--upstream-tcp-nodelay` | `true` | 布尔值 | Upstream TCP_NODELAY 可选值：`true`, `false`。 |
