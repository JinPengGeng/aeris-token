# Issue 300：每日实际费用持久计数决策

状态：已完成源码调查和实施方案，尚未实施或验收。该项与 per-attempt hard
plan quota 分开；本文件不表示前门每日计数已完成。

## 已确认的问题

- Gateway 的 `record_finalized_daily_usage` 每次接到 completed 父记录都累加
  完整金额，无法区分 no-op 重放或真正增量。
- typed child Outcome 经 `worker::write_event_record` 提前返回，迟到收费
  不经过该 Gateway hook；failed/cancelled 的已知收费也漏计。
- Redis 独立 INCR/EXPIRE 和恢复 GET/SET 不与财务提交处于同一原子边界，
  重放、部分失败和恢复并发会产生丢计或重复计数。

## 采用的方案及边界

在现有 usage/attempt 资金事务内维护一条父请求的持久贡献记录，保存整数
actual、原 user/key/standalone 身份和固定记账时间。`.06 -> .13` 替换绝对
贡献，只增加 `.07`。普通 key 计 user 与 key；standalone 只计 key。

attempt 使用首次持久 `funds_admission_closed_at` 作为日归属；close 前已知
收费先保存，close 后进入当日合计，迟到费用仍归原日。Unknown hold 不当成
已知成本；已知费用不依赖 completed 状态或钱包是否已经收齐。

前门以现有自然日窗口查询数据库中的 user/key 整数合计，停止依赖 Redis
增量和恢复值。保留现有 fail-open 与并发检查语义，因此这不是 hard quota
预占。仅启用每日限制的请求需要数据库读取；上线前须记录索引查询 EXPLAIN
和代表性延迟。若实测不达标，再考虑同事务日聚合。

贡献不随 usage/audit/outbox 清理删除。原请求身份、重放修订和归档后拒绝
身份复用必须有持久依据。迁移需要幂等 backfill、锁后重读以及排空旧 writer
再切读的可执行过程，不能把已删除财务证据的历史恢复宣称为完整。

## 必须补齐的验收

覆盖重复父/子事件、迟到收费、failed/cancelled、close 前后顺序、跨日及
DST、普通/standalone 归属、并发和事务回滚、backfill 与实时写入竞争、
清理后重放、Redis 故障无影响、数据库故障保持现有行为，以及作用域查询
性能。当前只有调查证据，没有这些实现与实测结果。
