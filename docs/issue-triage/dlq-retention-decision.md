# Issue #223: DLQ 保留策略决策

日期：2026-09-12

## 核验结论

Issue #223 的原始报告把 DLQ、备份恢复、任务重试、worker 监督和生命周期治理混在一起。代码核验确认备份已有 `aether-backup-restore` 恢复链路，且用量 worker 已具备 Redis/Memory 的原子死信转移和长度观测；因此本记录只覆盖仍真实存在的 DLQ 残项：此前写入没有有限保留上限，且没有管理端 redrive/query 操作出口。

DLQ 无上限会让 poison message 长期堆积并把 Redis 内存风险转化为计费运维风险。收益高、实现复杂度中等。redrive 属于高影响管理动作，需要单独定义权限、审计、幂等键、批量上限和失败语义，不能与容量保护混成一次改动。

## 本次实施

- `UsageRuntimeConfig.dlq_stream_maxlen` 默认 `50000`，启用配置时必须为正数。
- 网关增加 `AETHER_GATEWAY_USAGE_QUEUE_DLQ_MAXLEN`（默认 `50000`）。
- 普通死信追加使用近似 `MAXLEN`；内置 Memory 与 Redis 的原子 pending 转移也在同一转移操作中应用目标上限。
- Redis Lua 脚本先完成 PEL、类型、ACL 和上限参数校验，再执行 `XADD`、`XACK`、`XDEL`；上限淘汰最旧条目，不提前确认源消息。
- 保留 `push_dead_letter` 公开追加接口和第三方后端兼容行为；未实现有界原子 trait 扩展的外部后端仍沿用自身追加语义。

## 验收

- 配置值为 `0` 时启动校验返回 `InvalidConfiguration`。
- Memory 追加超过上限后 stream 长度保持上限。
- Memory 原子转移超过上限后仍只保留最新条目，且源 pending 在同一操作中确认并删除。
- Redis 原子转移继续使用 RESP2/RESP3、ACL 预检和重复调用幂等测试；新增上限参数不改变源未完成时的失败语义。

## 后续拆分

另开 PR 实现管理员 DLQ 查询和 redrive：只允许 `admin:usage:admin`，每次请求有数量/字节上限，按源 stream、consumer group、pending ID 做幂等转移，成功/跳过/失败逐项返回并写入 durable audit。redrive 不能绕过计费幂等或把已不在 PEL 的条目标记为已归档。另行建立 Postgres restore 演练和任务重试策略记录。
