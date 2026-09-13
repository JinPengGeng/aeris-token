# Issue 419: Public control-context deadline

## Scope

`ai_public` requests now have one asynchronous deadline after route
classification. The deadline covers the complete `ModelDirectivePolicySnapshot::load`
and `resolve_control_decision_auth_with_trusted_auth` future, including every
database/cache/gate await reached by those calls. Admin routes, admin writes,
upstream execution, request-body buffering, and response streaming remain outside
this budget.

The production setting is `AETHER_GATEWAY_CONTROL_CONTEXT_TIMEOUT_MS`. It is a
positive millisecond duration, defaults to `30000`, and is clamped to
`1..=120000` by the existing gateway duration parser. Tests use the
instance-local `AppState::with_public_control_context_timeout_for_tests` override
so process environment mutation cannot race parallel tests.

## Timeout contract

When the deadline expires, the resolver returns `GatewayError::ControlUnavailable`
with the request's original trace ID. The HTTP boundary therefore emits the
existing safe `502` response with `error.code=control_unavailable` and
`Retry-After: 1`; internal repository details are not exposed. The timed future is
dropped, so no command or authorization lookup is replayed by the timeout itself.

## Cancellation and recovery

The policy reads use the existing system-config singleflight guards. Auth context
loads use the existing auth-context and auth-snapshot cache flights, and snapshot
admission uses the existing RAII `ConcurrencyPermit`. Dropping the timed future
therefore removes the current leader flight (or releases a follower wait), wakes
followers, and returns gate capacity. Generation checks prevent a cancelled or
invalidated load from publishing stale data. Existing best-effort API-key
`last_used` updates retain their prior semantics and are not part of a new
transaction.

This deadline is a control-context phase budget, not a hard real-time CPU
preemption guarantee and not a replacement for database statement/lock timeouts.

## 验证与交付记录

父项为 #214，保持开放。工作树已同步主干 `30b09bd476a5a4337b31f560a28c1c7e77651644`，
包含 #415 的缺凭据 401 和 #416/#418 性能切片；没有修改管理员写入、资金或流式协议。
零值、非法字符串和未配置值沿用既有 duration parser 的默认 30000ms，不能用零关闭该期限。

作者新增两个真实 HTTP Router 回归，已实际执行 2 passed / 0 failed / 0 ignored：
策略缓存 follower 挂起时有界 502 与恢复；并发认证读取取消时有界 502、auth load gate
回到基线及后续读取恢复。另行复跑既有 API key carriers 目标 1 passed。首次编译和
格式调整不计为行为通过。作者超过原交还时间后，主线程接手最终集成与验证，原工作不丢弃。

独立源码评审 Accepted，确认 public 阶段边界、既有安全错误和 RAII 取消路径。
评审指出认证测试的 5ms 调度间隔不能严格证明两个 HTTP 请求分别占据 leader/follower：
该回归的证据是并发请求受限、取消后 gate 恢复与后续认证读可完成；不把它单独作为
两种 singleflight 角色均实际出现的证明。策略 follower 由显式持有 cache leader 的 fixture
建立，通用 singleflight 机制仍由已有 cache 回归覆盖。

独立脚本评审 Accepted，bash 语法、ShellCheck 与 diff 检查通过。最初脚本评审因超出
有界范围停止，未取得的结论不计入验收；替代评审只检查实际脚本和错误/许可断言。
主线程修正了手工添加环境参考行产生的生成漂移，使用既有 generator 重建后检查通过。

真实 PostgreSQL/Redis 的旧失败对照及候选成功结果见
[PostgreSQL 演练记录](issue-419-postgres-drill.md)。最终 head 的本地集成结果和四项
required checks 以 PR 后续验收记录为准，未完成门禁时保持 Draft，禁止绕过保护合并。
没有数据迁移；需要回滚时 revert 本切片，恢复原控制解析行为。
