# #255 管理员审计持久化核验与最小设计

核验快照：`12a1d265c`（2026-09-12）。本文只讨论 #255 的管理员审计持久化子项；
`/_gateway/audit/*` 鉴权与 Docker 升级策略是独立问题，不纳入本设计。

## 结论

当前管理员审计事件**可能被日志设施留存，但不会写入 `audit_logs`**：

- `emit_admin_audit` 只发出 `tracing` 的 `info!`/`warn!` 事件
  （`apps/aether-gateway/src/audit/admin.rs`），没有调用数据层。
- 网关 subscriber 只有格式化输出层。它可按配置写 stdout、滚动文件或两者
  （`crates/aether-runtime/base/src/tracing.rs`），没有把 `log_type = "audit"`
  转换为 repository 写入的 Layer。
- 默认 Compose 把网关日志写 stdout，再由 Docker `json-file` 驱动轮转；
  `release-local` 还可把文件日志写入 `/opt/aether/logs` 的持久卷。因此“只写
  stdout”不准确，但这些 sink 都不支持审计页面按用户、事件类型查询。
- `AuditLogReadRepository` 只有查询和删除方法，`PostgresAuditLogReadRepository`
  也只有 `SELECT`/`DELETE`。当前树没有 `INSERT INTO audit_logs`，
  `DataWriteRepositories` 也未安装 audit writer。
- 现有非阻塞日志 writer 在队列满、字节上限、单事件过大、sink 关闭或写失败时
  允许丢事件，只用 `logging_*` 指标报告；退出时最多等待 2 秒 flush，且明确不保证
  `fsync`（`crates/aether-runtime/base/src/tracing/writer.rs`）。运行时 `RUST_LOG`
  还可以过滤 `info` 级成功事件。这些语义不能作为 durable audit contract。

因此，#255 的“管理员审计未落库”子项成立；“`audit_logs`/页面在所有环境必为空”
不能由当前源码推出，因为历史导入或外部数据写入仍可能留下记录。准确表述应是：
**当前网关产生的 `emit_admin_audit` 事件不会新增可查询的 `audit_logs` 记录。**

## 推荐语义

不要在 tracing subscriber 中实现数据库桥接。subscriber 会受日志过滤影响，写库失败
时再次记录日志也容易形成递归，而且会把通用可观测性组件与网关数据库 schema 耦合。
应把结构化事件构造与 tracing 展示分开，并通过显式 repository 写入。

最小可接受实现采用“响应返回前直接异步写入”，不增加新的内存队列：

1. `emit_admin_audit` 拆为纯构造函数和现有 tracing 输出函数；只有已解析出
   `admin_principal`，且请求是管理员 mutation 或显式附加审计事件时才构造记录。
2. 在最终响应路径中，对构造出的记录调用 `AuditLogWriteRepository`，并在一个小于
   HTTP shutdown deadline 的显式超时内 `await`。记录成功落库后才把原业务响应交给
   Hyper。
3. repository 使用调用方预先生成的 UUIDv7 作为 `id`，以相同 ID 和
   `INSERT ... ON CONFLICT (id) DO NOTHING` 保持有限重试幂等，并让返回值区分
   `inserted` 与 `duplicate`。
4. tracing 事件继续保留，作为操作日志和落库失败时的旁路告警，但不再被描述为审计
   数据的真源。

该方案会增加一次管理员请求的数据库往返，但管理员操作量低，且它比另建一个可能在
进程崩溃时丢失的内存队列更符合“持久化”目标。不要用 fire-and-forget 冒充 durable。

### 复用现有 schema

不新增表，直接复用 `audit_logs`：

| 列 | 写入值 |
| --- | --- |
| `id` | 调用方生成的 UUIDv7 |
| `event_type` | 低基数固定值 `admin_mutation` 或 `admin_sensitive_read` |
| `user_id` | `admin_principal.user_id` |
| `api_key_id` | `NULL`；管理 token ID 不是 API key ID，不应混用 |
| `description` | 固定、短且无请求内容的说明，例如 `admin action: {action}` |
| `ip_address` | 已解析的可信 `client_ip` 字符串 |
| `user_agent` | 首版为 `NULL`；除非先定义截断和敏感信息规则，不复制原始 header |
| `request_id` | 当前 `trace_id`，用于关联请求日志 |
| `event_metadata` | 下述版本化、白名单 JSON |
| `status_code` | 最终 HTTP 状态码 |
| `error_message` | 首版为 `NULL`；不得落响应 body、数据库错误或堆栈 |
| `created_at` | repository/数据库写入时刻 |

现存基线迁移把 `event_type` 定义为 `varchar(50)`，逻辑 schema 则声明 64 字符；定向扫描
发现的管理员静态 event name 最长已有 59 字符。首版不能直接把 event name 填进该列，
否则旧部署会写入失败；使用上述短分类值，把精确名称放进 metadata，也避免本次为持久化
额外扩大 schema 变更。

`event_metadata` 只允许这些键：`schema_version`、`event_name`、`status`、`admin_role`、
`session_id`、`management_token_id`、`route_family`、`route_kind`、`method`、
`path`、`action`、`target_type`、`target_id`、`target_truncated`。其中：

- `path` 必须继续经过 `sanitize_admin_audit_path`，去除 token、API key 等 query 值；
- path 形态的 `target_id` 继续复用 `sanitize_admin_audit_target_id`；所有 target 均设长度
  上限，超限时截断并附 `target_truncated = true`，不要把请求/响应 payload 写入 metadata；
- `session_id` 是数据库会话记录 ID而不是 cookie；`management_token_id` 是记录 ID而不是
  token secret。若来源不满足这一约束则写 `NULL`；绝不保存 Cookie、Authorization、
  management token permissions、redeem code、provider credential、请求体或响应体；
- JSON 必须由强类型 `CreateAdminAuditLog` 序列化，不接收任意调用方 metadata。
- 继续复用现有 audit cleanup：默认 30 天、最低 7 天、分批删除；IP 和会话关联标识不得
  绕过这套 retention。若合规要求更长留存，应通过已有系统配置显式决定。

### 写失败行为

管理员 mutation 的业务副作用通常在 finalizer 之前已经提交。审计写失败后把原本成功
的响应改成 5xx 会诱导客户端重试并可能重复副作用，所以首版应采用：

- 保留原业务响应和状态码；
- 对审计 INSERT 设置短超时，不做无限重试；
- 发出独立的 `error!` 事件 `admin_audit_persist_failed`，只带事件 ID、trace ID、
  route/action 和脱敏后的错误类别；
- 增加 `admin_audit_persist_attempts_total`、`admin_audit_persist_failures_total`、
  `admin_audit_persist_timeouts_total` 指标，并对 failures/attempts 告警；
- 数据库已配置但 audit writer 缺失属于 wiring 缺陷，启动检查应 fail closed；完全无数据库
  的开发模式明确报告 `durable_admin_audit_available = 0`，不能返回“持久化成功”。

这不是严格合规模式。严格模式不能在副作用完成后才失败；它需要在同一事务内写业务变更
和审计记录，或使用事务 outbox。跨 repository、远程 provider 和系统升级动作无法用一次
简单 INSERT 获得该保证，应另写 ADR 后分阶段实现。

### Shutdown

直接 `await` 写入不需要新的 audit worker 或 queue drain。网关现有 shutdown 会先等待 HTTP
连接/请求（默认 30 秒），所以 finalizer 内的 audit INSERT 会包含在请求 drain 中；每次
INSERT 的超时必须显著小于该 deadline。随后现有 `LogShutdownGuard` 仍只负责 tracing
writer 的 2 秒 drain，两者不得混为一谈。

若后续为了吞吐改为队列，必须同时增加显式 `AdminAuditRuntime::shutdown(deadline)`：停止
接收、耗尽所有已接收事件、等待 INSERT 完成，并在 `gateway_shutdown_complete` 之前检查
结果。没有该生命周期的后台任务不应合并。

## 验收测试

1. **Repository/Postgres 集成**：写入一条 `CreateAdminAuditLog`，通过现有
   `list_admin_audit_logs` 按 `event_type` 查询；校验 UUID、用户、状态、metadata 和时间。
2. **成功 mutation**：执行一个低副作用管理员 mutation，断言业务响应成功且恰好新增
   一条对应事件；再用相同 event ID 写入，断言不会重复。
3. **拒绝/失败 mutation**：执行权限拒绝或校验失败路径，断言事件状态、HTTP code 和
   principal 正确，且不含 Authorization/Cookie/请求 body。
4. **脱敏**：path/target 中放入 `token`、`api_key`、one-time secret、redeem code 和长值，
   断言数据库的所有文本列及 JSON 序列化结果均不包含原值。
5. **写失败**：使用 failing repository 和 timeout repository，断言原业务响应不被改写、
   failure/timeout 指标各增加一次、告警事件不含底层错误中的模拟 secret。
6. **wiring**：Postgres backend 同时提供 audit reader/writer；数据库配置存在但 writer
   缺失时启动失败，无数据库测试模式明确暴露不可持久化状态。
7. **shutdown**：让测试 repository 阻塞 INSERT 后触发 shutdown，释放 INSERT 后断言网关
   在 deadline 内等待并保存记录；另测超时能终止 drain 并增加 timeout 指标。
8. **查询闭环**：通过受保护的管理员 audit API 读到刚写入的 mutation，普通用户和权限
   降级的 management token 仍不能读取。

## 建议写集

- `crates/aether-data/contracts/src/repository/audit.rs`：新增强类型输入和
  `AuditLogWriteRepository`。
- `crates/aether-data/adapters/postgres/src/audit.rs`：在现有 audit repository 上实现 INSERT。
- `crates/aether-data/runtime/src/backend/write.rs`：安装并暴露 audit writer，同时补 wiring 测试。
- `apps/aether-gateway/src/data/state/{mod.rs,core.rs,runtime.rs}`：保存 writer，并提供单一写入口。
- `apps/aether-gateway/src/audit/admin.rs`：构造强类型记录、集中脱敏并保留 tracing 输出。
- `apps/aether-gateway/src/handlers/proxy/finalize.rs` 及其直接调用点：把 finalizer 改为可等待的
  异步边界；不要在各管理员 handler 中重复写库。
- 对应 contracts、Postgres、finalizer、operational-auth 和 shutdown 测试文件。

首个 PR 不应修改 schema、引入通用 tracing-to-database Layer、增加无限内存队列，或把
Docker 升级问题混入同一写集。
