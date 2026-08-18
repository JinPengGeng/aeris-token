# ADR-0046: Attempt 与 Client Commit 生命周期

- 状态：Accepted for MVP core
- 日期：2026-08-18
- 关联：Issue #46

## 背景

同步 HTTP、流式 HTTP 与 Responses WebSocket 都可能在一次 logical request 中执行多个 physical upstream dispatch。若 retry 判断只依赖 transport 错误或 candidate 状态，在响应已经进入不可回滚的客户端交付阶段后仍切换 candidate/credential，会重复执行上游请求，尤其会放大 tool call、Compact 或其他副作用操作。

现有 `ExecutionAttemptLifecycle::begin`、`mark_started`、`settle` 管理 usage、candidate、provider effect 与 execution report 的记账顺序。它不是 replay-safety 状态机，本 ADR 不改变其兼容语义。

## 决策

新增 transport-neutral 的 `AttemptDispatchLifecycle`。一个实例只代表一次 physical dispatch，初态为 `Prepared`，状态只能沿以下边迁移：

| 当前状态 | 允许的下一状态 |
|---|---|
| `Prepared` | `SentButUncommitted`、`Terminal` |
| `SentButUncommitted` | `ClientCommitted`、`Terminal` |
| `ClientCommitted` | `Terminal` |
| `Terminal` | 无 |

`mark_sent` 只能调用一次。同一 lifecycle 不得表示第二次 physical dispatch。需要 retry 时，旧 lifecycle 必须先通过 `settle_for_replay` 进入 `Terminal`，其返回的单次 `ReplayPermit` 才能创建一个新的 `Prepared` lifecycle。

### ClientCommitted

`ClientCommitted` 表示 gateway 已完成不可回滚的应用层 response handoff。它不声称客户端实际收到了数据，也不等同于 provider terminal、usage settled 或 socket write 成功。

各 transport adapter 后续必须在自身真正不可回滚的边界调用 `mark_client_committed`。Responses WebSocket 的具体 public-send 事件与并发顺序不在本 core slice 内决定，必须在接线 PR 中用 transport 测试固定。

### ReplayBarrier

执行事实状态与 replay barrier 分离：

- 状态回答“这次 physical dispatch 已经走到哪里”。
- barrier 回答“即使尚未 client commit，是否仍允许 replay”。

barrier 初始为 `Open`，只能单调关闭。Compact、tool call、已知副作用请求或 ambiguous dispatch outcome 可以在 `SentButUncommitted`（也可更早）关闭 barrier。关闭 barrier不会伪造 `ClientCommitted`，第一个关闭原因保留，之后的关闭观察幂等。

### ReplayPermit

只有同时满足以下条件，`settle_for_replay` 才签发 permit：

1. 状态仍为 `Prepared` 或 `SentButUncommitted`；
2. barrier 为 `Open`；
3. 当次 retry policy 决策为 `Allow`；
4. 当前 lifecycle 尚未签发 permit。

permit 在旧 lifecycle 迁移到 `Terminal` 之后才返回给调用方。permit 不实现 `Clone`，并在 `start_next_attempt` 中记录消费状态；重复消费 fail closed。`ClientCommitted`、已关闭 barrier、policy deny、重复签发或任何 terminal 后操作均拒绝。

## 兼容与接线

- 本次只增加 core model 与测试，不接线 candidate loop、sync、stream 或 WebSocket。
- 现有记账 `begin`、`mark_started`、`settle` 的签名和行为保持不变。
- 后续 adapter 应让 replay-safety lifecycle 包住一次真实 dispatch，再把现有记账 lifecycle 作为正交的持久化/效果流程使用。
- 所有异常与不明确交付结果默认关闭 replay barrier，除非 transport 能证明仍可安全 replay。

## 范围限制

MVP 只保证同一进程、同一 logical request 内的状态与 permit 所有权。它不提供：

- 跨进程恢复或多实例互斥；
- 持久化 idempotency ledger；
- gateway 重启后的 replay 判定；
- 上游 provider 的端到端 exactly-once 保证。

这些能力需要持久化 attempt identity、原子 ledger 与 provider idempotency contract，不能从内存状态机推导。

## 验证

单元测试固定以下不变量：完整转换矩阵、禁止回退、禁止二次 dispatch、terminal 后不可迁移、barrier 单调关闭且不伪造 commit、policy/commit/barrier 三重 replay gate、permit 单次消费、每个 replay 创建新 lifecycle，以及 settlement 只能发生一次。
