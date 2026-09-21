# Redis 故障降级语义与“一致性优先”全局开关

状态：Accepted（2026-09-21；开关已实现，默认行为不变）

## 背景

Issue #224 评审指出：Redis 是唯一的跨节点协调层，而多处运行时在 Redis 故障时
采取 fail-open 或进程内降级语义——Redis 一抖，全集群限额短暂失效（多节点限额 ×N、
日用量 fail-open）。改进建议要求提供“一致性优先”全局开关，并先出决策记录。

## 现状核验（本决策基线）

| 路径 | Redis/共享状态故障时的默认行为 |
| --- | --- |
| Frontdoor 用户 RPM 限速（`apps/aether-gateway/src/rate_limit.rs`） | `RATE_LIMIT_FAIL_OPEN` 默认 `false`（已 fail-closed）；但 `allow_local_fallback` 默认 `true`，单节点拓扑下退化为进程内计数。多节点拓扑在 `main.rs` 启动期已强制关闭 local fallback。 |
| 日用量限额（`apps/aether-gateway/src/daily_usage_limit.rs`） | 检查失败时固定 fail-open（`Allowed` 并记 `billing_fail_open_daily_quota`），无任何开关。 |
| 31 类单例后台任务、usage Stream 队列、分布式信号量 | 走 Redis 租约/fencing/消费组；Redis 不可用时任务停摆或降级，不放大限额，不在本开关范围。 |
| 进程内 TTL 缓存（auth_context、system_config 等） | 失效窗口由 TTL 决定，与 Redis 可用性无关；跨节点即时失效需另行实现 pub/sub，不在本开关范围（见 [gateway-cache-consistency](gateway-cache-consistency.md)）。 |

## 决策

新增全局开关 `AETHER_CONSISTENCY_FIRST`（CLI `--consistency-first`，默认 `false`）。
开启时：

1. RPM 限速禁用 local fallback（`with_local_fallback(false)`）——无论拓扑，
   Redis 不可用时按 `RATE_LIMIT_FAIL_OPEN` 的语义处理（默认拒绝），
   不再退化为每节点独立计数，消除限额 ×N。
2. 日用量限额检查在共享状态故障时 fail-closed：返回新的
   `FrontdoorDailyUsageOutcome::Unavailable`，由候选执行入口拒绝请求
   （HTTP 429 + `Retry-After`，`X-Daily-Usage-Scope: runtime_unavailable`），
   并保留 `billing_fail_open_daily_quota` 指标计数用于告警。

默认（开关关闭）行为与现状完全一致：RPM 保持 fail-closed + local fallback，
日用量保持 fail-open。

## 理由

- 多节点拓扑下 local fallback 已在启动期被强制关闭，×N 风险主要残留在
  “单节点拓扑 + 可选 Redis”的部署和日用量 fail-open；全局开关让运营方在
  一致性敏感场景（生产多节点、对外配额承诺）用单一环境变量收敛全部降级语义。
- 拒绝实现 Redis 故障时的“限额冻结”中间态（沿用最后一次已知计数）：需要
  跨进程持久化计数快照，复杂度高且引入新的不一致面，收益不成比例。
- 进程内缓存的跨节点失效窗口是独立问题（TTL 语义，非故障降级），保持
  文档化边界，不由本开关覆盖。

## 实现与验证

- 开关接线：`apps/aether-gateway/src/main.rs`（`GatewayRateLimitArgs.consistency_first`）；
  限流器语义：`rate_limit.rs` 既有 `with_local_fallback`；
  日用量 fail-closed：`daily_usage_limit.rs` 的 `with_fail_open(false)` +
  `FrontdoorDailyUsageOutcome::Unavailable`；
  消费端：`executor/candidate_loop.rs` 合成拒绝对象；
  状态接线：`state/core.rs` 的 `with_frontdoor_daily_usage_fail_open`。
- 测试：`daily_usage_limit.rs::consistency_first_denies_when_usage_repository_is_missing`
  验证 fail-closed 分支；既有 `missing_usage_repository_reports_an_error_and_checks_fail_open`
  验证默认分支不变。
- 定向验证：`cargo test -p aether-gateway daily_usage_limit`、`cargo check -p aether-gateway`。

## 未完成范围

- 进程内 TTL 缓存的跨节点失效通道（pub/sub 或版本检查）仍未实现；
  失效窗口以 [gateway-cache-consistency](gateway-cache-consistency.md) 为准。
- 开关未在多节点 compose 模板默认开启；是否默认开启属于部署策略决定，
  当前仅文档化（见 `docs/operations/multi-node-deployment.md`）。
