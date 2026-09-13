# Usage core/runtime 与重试、DLQ

状态：Accepted。日期：2026-09-13。源码基线与维护规则见 [ADR 索引](README.md)。
关联：Issue #235、#223。

## 背景与分层决定

用量事件需要在写入延迟、进程退出与后端故障之间保留准确的计费事实。接受现有
runtime 的有界重试、成功后确认、永久错误进 DLQ 与受控 redrive；不把消息交付称为
端到端 exactly-once。

[aether-usage-core](../../crates/aether-usage/core/src/lib.rs) 是仅依赖 serde/thiserror
的纯合同：事件身份校验、subject 分区与 `SettlementDisposition` 等类型。
[runtime](../../crates/aether-usage/runtime/src/lib.rs) 则实现事件编解码、入队/消费、
富化与持久化，依赖数据和运行时后端。

这个边界目前只是部分拆分：[runtime Cargo.toml](../../crates/aether-usage/runtime/Cargo.toml)
没有依赖 core，runtime [event.rs](../../crates/aether-usage/runtime/src/event.rs)
定义自己的私有 wire envelope，不能将它与 core 的同名类型混为一谈。core 声明的
`AlreadyApplied` 或 `RejectOutOfOrder` 不是生产结算已采用该状态机的证据。
将所有事件/结算策略迁入 core 是未来方案，当前 ADR 不授予其已接线状态。

## 当前生命周期

| 阶段 | 接受的行为 | 故障边界 |
| --- | --- | --- |
| 生产者入队 | runtime 使用有界本地队列、按 request 分片的 retry worker 与封顶退避 | 本地缓冲不是持久日志；容量、永久输入错误和停机失败需要显式观测 |
| consumer 解码与富化 | 保留原始 stream fields；先完成终态富化，再调用 writer | 无法解码或已分类永久错误走 DLQ；富化失败不能当成成功的零费用事件 |
| writer 成功 | worker 随后 ACK 并删除源消息 | 写成功但 ACK 失败可导致重投，幂等必须由实际数据写入/结算路径提供 |
| 可重试写错误 | 不 ACK，留在 pending，后续 reclaim 重试 | 暂时故障不因固定次数耗尽自动降格为成功 |
| 永久错误 | 内置后端原子转移到 DLQ 后确认源消息 | 编码预算不足保留 pending；第三方后端兼容回退不具备同等原子保证 |
| 管理员 redrive | 从服务端保存的原始字段重放，成功删除 DLQ 条目，重复请求返回原目标 ID | 返回 redriven 仅代表入队；仍须检查后续消费、结算和再次死信 |

[runtime.rs](../../crates/aether-usage/runtime/src/runtime.rs) 的 enqueue retry worker
对永久入队错误终止当前 item 并继续下一项，临时失败按指数退避重试。
[配置](../../crates/aether-usage/runtime/src/config.rs) 的库默认值为 8 个 retry worker、
131072 个缓冲位置、3 秒初始/10 秒最大退避；这是入队失败的本地重试，和 consumer 的
pending reclaim（默认 idle 60 秒、检查间隔 5 秒）不同，也不是上游收费请求重发策略。
生产者路径还有直接持久化与终态返回值，不能从“可重试”推断每次提交都已可靠落盘。

[worker](../../crates/aether-usage/runtime/src/worker.rs) 将 InvalidConfiguration、
InvalidInput、UnexpectedValue 及已识别的数据库外键错误视为永久错误；Redis、timeout
及其余未识别数据库错误可重试。原始 fields 保留到写入/死信完成；解码事件的 body
预算处理不应改写死信所需的原始计费输入。

## DLQ 取舍与操作约束

[队列实现](../../crates/aether-usage/runtime/src/queue.rs) 使用内置 Memory/Redis 的
原子 pending 转移能力。默认 DLQ 上限 50000，启用时要求正数；Memory 精确限制长度，
Redis `MAXLEN ~` 近似裁剪最旧条目。有限保留保护内存，但会丢弃超过窗口的旧失败事件，
它不是永久账务审计库。第三方后端未实现原子扩展时保留原追加/确认兼容语义。

[管理入口](../../apps/aether-gateway/src/handlers/admin/observability/usage/dlq.rs)
提供游标列表（默认 50、最多 100）及单条 redrive，沿用管理员授权与审计链。
列表不直接返回完整事件体；redrive 不接受客户端注入替代事件字段。
已合并的权限/保留决定见 [DLQ 决策](../issue-triage/dlq-retention-decision.md)。
[Redis redrive Lua](../../crates/aether-runtime/state/src/redis/dead_letter_redrive.lua)
先校验 ACL/类型，再追加目标、记录幂等 marker、删除源条目；要求 Redis 7 的 ACL 预检。

当前 Redis marker 使用无 TTL 的 SET，Memory 的
[marker map](../../crates/aether-runtime/state/src/memory.rs) 也没有到期策略。
幂等返回依赖 marker 保留和后端状态存活，不能把 DLQ MAXLEN 当作 marker 容量上限。
marker 的寿命/清理与容量政策仍待设计；未经对账不得直接删 marker 或 DLQ。

## 兼容、回滚与未完成范围

runtime wire envelope 使用显式版本字段；未知版本被拒绝并交给 DLQ 路径，而非猜测
解析。旧 payload 中缺省字段、显式 capture 状态和 legacy metadata 的兼容由 decoder
与相关测试约束。修改 wire schema 时，需先验证旧积压与新消费者，并确认回滚版本能
读取升级后消息；不能仅回滚二进制后盲目 redrive。

消费故障先保留 pending/DLQ，修复后端或消费者，再通过管理入口重放并核对结算。
使用 [Redis 运行手册](../operations/redis-runtime-runbook.md) 选择持久化 profile；
Memory、本地 retry 缓冲及无持久化 Redis 都不能承诺进程/主机重启后的恢复。
库的停机 drain 与 Redis 已确认入队的语义见
[停机测试](../../crates/aether-usage/runtime/src/runtime_shutdown_tests.rs)，不将 drain 超时
解释为业务全部成功。

本 ADR 不完成 core 的生产接线、所有任务类别的 retry 政策、marker 容量/TTL、批量
redrive、生产容量与完整账务灾备验收，也不描述未合并 PR #391 的 attempt 资金生命周期。
runtime 与数据仓储评审角色负责消息/写入合同，管理员入口评审角色负责权限与审计，
部署责任人负责留存容量、告警和业务对账。

## 可复核证据

- [core 身份校验](../../crates/aether-usage/core/src/record.rs)：`envelope_rejects_missing_identity`。
- [worker 回归](../../crates/aether-usage/runtime/src/worker.rs)：
  `usage_event_record_error_classifies_permanent_failures`、
  `capture_budget_retry_releases_decoded_lease_and_preserves_pending_payload`、
  `process_entries_dead_letters_permanent_record_error_and_continues`。
- [重试 payload 回归](../../crates/aether-usage/runtime/src/runtime_queue_payload_tests.rs)：
  `retry_worker_discards_oversize_and_drains_next_event_on_the_same_shard`；
  [wire 回归](../../crates/aether-usage/runtime/src/event_wire.rs)：
  `event_wire_preserves_explicit_capture_states_and_legacy_metadata`。
- [真实 Redis redrive 测试](../../crates/aether-runtime/state/src/redis/dead_letter_redrive_tests.rs)：
  `redis_dead_letter_redrive_is_idempotent_and_retention_bounded`，复现入口与环境限制见
  [DLQ 演练记录](../issue-triage/dlq-recovery-drill.md)。

本次只读取代码和断言，未重跑 Rust 或 Redis 测试。既有 fixture 不可用时可能跳过；
测试名称或一条 redrive 成功不能替代完整生产消费与结算对账。
