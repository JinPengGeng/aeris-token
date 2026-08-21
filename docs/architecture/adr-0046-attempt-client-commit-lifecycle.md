# ADR-0046: Attempt 与 Client Commit 生命周期

- 状态：Accepted and wired for in-process transports
- 日期：2026-08-18
- 关联：Issue #46

## 背景

同步 HTTP、流式 HTTP 与 Responses WebSocket 都可能在一次 logical request 中执行多个 physical upstream dispatch。若 retry 判断只依赖 transport 错误或 candidate 状态，在响应已经进入不可回滚的客户端交付阶段后仍切换 candidate/credential，会重复执行上游请求，尤其会放大 tool call、Compact 或其他副作用操作。

现有 `ExecutionAttemptLifecycle::begin`、`mark_started`、`settle` 管理 usage、candidate、provider effect 与 execution report 的记账顺序。它不是 replay-safety 状态机，本 ADR 不改变其兼容语义。

## 决策

新增 transport-neutral 的 `AttemptDispatchLifecycle`，并由每个 logical request 唯一的 `LogicalRequestReplayOwner` 持有共享 `ReplayAuthority`。一个 lifecycle 实例只代表该 request 的一个、带 generation 的 physical dispatch，初态为 `Prepared`，状态只能沿以下边迁移：

| 当前状态 | 允许的下一状态 |
|---|---|
| `Prepared` | `SentButUncommitted`、`Terminal` |
| `SentButUncommitted` | `ClientCommitted`、`Terminal` |
| `ClientCommitted` | `Terminal` |
| `Terminal` | 无 |

`AttemptDispatchLifecycle` 不实现 `Default`，也没有 crate-visible 构造器。owner 只能签发一次 generation 0；后续 generation 只能由前一 generation 的单次 `ReplayPermit` 产生。丢弃或 `mem::forget` lifecycle、in-flight ownership 或 permit 都不能重建 owner、清除 in-flight fence 或推进 generation。

### ClientCommitted

`ClientCommitted` 表示 gateway 已完成不可回滚的应用层 response handoff。它不声称客户端实际收到了数据，也不等同于 provider terminal、usage settled 或 socket write 成功。

各 transport adapter 在自身真正不可回滚的边界调用 `mark_client_committed`：同步与流式 HTTP 由包装后的 response body 在首次 poll 前提交；Responses WebSocket 在 `send_client_message` 成功后提交。WS 的透明 quota replay 必须先关闭旧 socket、等待旧 attempt settlement，再消费 generation-bound permit。

### ReplayBarrier

执行事实状态与 replay barrier 分离：

- 状态回答“这次 physical dispatch 已经走到哪里”。
- barrier 回答“即使尚未 client commit，是否仍允许 replay”。

barrier 属于 logical request 的共享 authority，而不是某个 physical lifecycle。它初始为 `Open`，只能单调关闭。Compact、tool call、已知副作用请求或 ambiguous dispatch outcome 可以随时通过可克隆的 observation handle 关闭 barrier，包括旧 lifecycle 已经 `Terminal` 之后。关闭不会伪造 `ClientCommitted`，第一个关闭原因保留，之后的关闭观察幂等。

permit 记录签发时的 request identity、generation 与 barrier revision。迟到事实在下一 generation 创建前会使 permit stale；若下一 lifecycle 已处于 `Prepared`，`mark_sent` 仍会重新读取共享 barrier，保证迟到事实发生在 send admission 之前时 fail closed。

### ReplayPermit

只有同时满足以下条件，`settle_for_replay` 才签发 permit：

1. 状态为 `SentButUncommitted`，尚未 client commit；
2. shared barrier 为 `Open`；
3. opaque policy approval 与当前 request、generation、barrier revision 完全匹配；
4. opaque quiescence proof 与当前 request、generation、authority identity 完全匹配；
5. shared authority 仍把该 generation 记录为唯一 in-flight dispatch；
6. 当前 generation 尚未签发 permit。

permit 在一次原子 authority 更新中清除旧 in-flight generation、写入单调 fence 并把旧 lifecycle 迁移到 `Terminal` 后才返回。permit 不实现 `Clone`，并在消费时验证 authority identity、旧 generation、fence、barrier revision 与无 in-flight dispatch；重复或 stale 消费 fail closed。

policy approval 不是调用方可以直接构造的 `Allow` 枚举；quiescence proof 也不能由调用方声明一个布尔值获得。两者均为 sealed opaque capability。

### Quiescence 与 adapter surface

纯状态机无法证明一个 transport future 已 join，或 cancel 后已观察到终止。因此 sibling module 仍不能访问 owner 构造器、policy issuer、quiescence confirmer、`settle_for_replay` 或 permit consumer；只能调用 `AttemptReplayHandle` 的 transport adapters。

HTTP `Retry` 只能由完整收集且已经过 typed failure-origin classifier 的 attempt 返回；WebSocket quota retry 只有关闭旧 socket并等待旧 finalizer 后才能授权。timeout、future drop、client body drop、serialization failure 或任何不明确交付均关闭 `AmbiguousDispatchOutcome` barrier，不签发 replay permit。

## 兼容与接线

- candidate loop 的 sync/stream port 共享 logical-request replay handle；每次真实发送前进入 `SentButUncommitted`，retry 必须先消费上一 generation 的授权。
- Responses WebSocket turn 在 upstream writer 接受 `response.create` 后进入 `SentButUncommitted`，首个 public frame 成功写给客户端后进入 `ClientCommitted`；quota retry 复用同一 authority。
- 现有记账 `begin`、`mark_started`、`settle` 的签名和行为保持不变。
- 后续 adapter 应由 canonical logical-request owner 创建唯一 replay authority，让 replay-safety lifecycle 包住一次真实 dispatch，再把现有记账 lifecycle 作为正交的持久化/效果流程使用。
- 所有异常与不明确交付结果默认关闭 replay barrier，除非 transport 能证明仍可安全 replay。

## 范围限制

当前实现定义并验证同一进程、同一 logical request 内的状态、generation、fence 与 capability 所有权，并接线本地 sync/stream/Responses WebSocket 路径。它仍不提供：

- 跨进程恢复或多实例互斥；
- 持久化 idempotency ledger；
- gateway 重启后的 replay 判定；
- 上游 provider 的端到端 exactly-once 保证。

这些能力需要持久化 attempt identity、原子 ledger 与 provider idempotency contract，不能从内存状态机推导。

## 验证

单元与 compile-fail 测试固定以下不变量：完整转换矩阵、owner 只签发一个首 attempt、无 `Default/new` 绕过、sibling module 不能签发 policy/quiescence/permit、request/generation/fence 绑定、drop/forget 不清 fence 或推进 generation、迟到 barrier 并发撤销 permit、prepared replay 在 send admission 再检查 barrier，以及 permit 单次消费。
