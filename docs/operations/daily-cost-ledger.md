# 前门每日实际费用账本（#300）

决策日期：2026-09-13。实现基线 `33a0e7b7e5dffdc8c251d4dcd0f10325c96dee93`。

前门每日 user/key 限额读取 PostgreSQL `usage_daily_cost_contributions` 的已提交整数合计。原先 Gateway 在每次父 usage upsert 后向 Redis 累加整笔费用，重复投递会重复计数，迟到 child Outcome 又绕过这个入口。新的贡献与父 usage / child 资金事实在同一个事务提交，数据库提交后即对后续检查可见；队列重放只更新同一条绝对累计值。

本功能保持已有错误 fail-open、admin/IP bypass、未配置限额时不查询，以及并发前门检查语义。它统计已知实际费用；hard attempt plan quota 仍独立负责预占和结算。每日检查与后续执行不处于同一事务，因此这里没有宣称并发硬准入。

## 金额、身份和日界线

- 一个 `request_id` 一条贡献，关联原 `usage.id`。`.06` 已计父再收到 `.07` child 时，该行变为 `.13`，总计只增加 `.07`。
- attempt 使用持久 child 汇总 `known_actual_cost_units`，不经过父展示 f64。failed/cancelled 已知收费、超授权但已发生费用均计入；Unknown、hold、待收余额不当成实际费用。
- attempt 首次持久 `funds_admission_closed_at` 为记账 UTC 时间。close 前金额已保存但不入完成请求的日合计，close 后即使父展示事件尚未到达也可计入。D2 到达的迟到费用回补 D1 close 所属日，重复 close 和 parent revision 不移动时间。
- legacy 首次 accepted completed 的 finalized_at 固定为记账时间，缺失则用该次 updated_at；合法 completed 修订可向上或向下替换费用，失败展示事件不自动退款。
- 初始 user/key/standalone 归属冻结。普通 key 计 user+key，standalone 仅 key，没有 API key 的记录不加入前门 user 用量。后续 metadata/归属变动不能转移旧贡献。
- 前门用现有 APP_TIMEZONE 自然日函数查询 `[start,end)`，支持 23/25 小时 DST 日；账本保留 UTC 时间。重启修改时区会改变查询日范围，不产生第二笔贡献；所有实例必须使用一致时区配置。
- 贡献没有 audit/user/key 的 cascading foreign key，也不参与普通 usage 或 stats outbox 清理。审计删除不退款；已删除审计的 request ID 重放会被拒绝，不建立第二条身份。当前未新增自动账本清理，因为系统尚未定义安全的金融重放/修订期限。

## 必须采用的首次切换流程

这是排空旧 writer 后的升级，不支持新读与旧写任意混跑。只完成建表然后立即滚动切读会漏掉尚未升级实例写入的费用。

1. 停止接入新请求，排空所有旧 Gateway、usage worker 和资金写入者；保留已持久队列用于新版本重放。确认没有旧 writer 在迁移后继续写。
2. 使用已有部署环境和密钥配置运行新版本 `aether-gateway --migrate`。新迁移 `20260914030000_add_daily_actual_cost_ledger.sql` 建表并执行 backfill，迁移失败时不启动新前门。
3. backfill 对 usage 和账本取得排他表锁，并在同一事务按现存有效财务事实填充：legacy 从 settlement snapshot / usage 费用读取，attempt 从 child terminal facts 的整数读取，原 close timestamp 保留。已有贡献只替换金额与模式，保留最初身份和时间。
4. 所有服务使用新版本后恢复流量。旧 Redis daily keys 可自然过期，不需要删除 Redis 的其他数据。Redis FLUSH、局部驱逐、旧 ready 标记都不再影响每日判定。

需要重复核验/回填时，同样先排空旧 writer，然后在已有受控数据库连接中执行：

```sql
SELECT public.backfill_usage_daily_cost_contributions();
```

返回本次处理的 source 行数。重复执行不增加贡献。backfill 本身通过表锁与新 writer 串行化；排空仍是旧版本切换的必要条件。不要修改 SQLx migration checksum 或删除迁移账本来掩盖版本问题。

历史边界：若旧 usage 和唯一 child 财务证据在升级前已经被删除，无法从漂移的 Redis 数值恢复精确历史。升级前应确认当前自然日及未解决请求的财务源仍在。backfill 不对已经不存在的源宣称恢复成功，也不删除已经保存的独立贡献。

## 验证与性能证据

在任务独立 PostgreSQL 17（新数据库、全量真实迁移）验证了 legacy 并发重放/归属冻结，以及 attempt `.06 + .07 = .13`、晚到原日、父 failed/cancelled/completed 重投、close 重投、贡献校验失败导致 child/钱包/汇总一起回滚、audit 删除保留和 request ID 复用拒绝。另一个真实用例清空测试贡献以模拟迁移前 source，然后运行实际 backfill 函数并并发重跑，验证整数 source、原身份/日界线及 source 删除后的保留。批量 pending/first-byte 还验证冻结身份和整批 rollback；批量路径维持按批写入，不逐行解码完整审计。测试位于 `settlement/funding/attempts/daily_cost_tests.rs`，五个 exact targets 已加入 `tools/ci/run_postgres_live_tests.sh`。

Memory 对应规则使用共享 transition policy，已覆盖 scope、合法金额下调、stale lifecycle、晚到 child、保留隔离及失败不改父/费用。Gateway fixture 使用实际 memory repository，验证持久读取与 fail-open；测试不再通过直接注入 Redis 值建立“成功”。

本地 PG EXPLAIN：10 万条当日贡献、一个 user 匹配 1 万条、一个 key 匹配 1000 条。两个 scope 都选择对应时间/身份索引的 bitmap scan，同一 statement 完成；第一次查询 planning 0.294 ms，execution 6.884 ms。该记录使用临时表、本机 PG 和合成数据，不包含网络、连接池等待或生产并发，不能当作生产 P99 或容量承诺。实现不是 O(1)；高流量主体查询成本随当日记录数增长。新 stage `daily_usage_limit_persistent_read` 已注册，用于上线观察实际延迟；只有观测表明预算不足时再增加同事务日聚合，避免先引入新的 Redis 双写恢复协议。

实现过程中发现并修正：首版 backfill 错用不存在的 usage.updated_at，改为 updated_at_unix_secs/created_at；legacy backfill 需优先 settlement snapshot，与正常读取一致。主线程审查另发现 PostgreSQL 将小数 epoch 转 bigint 时四舍五入，可能使 23:59:59.9 跨日；读取改为 FLOOR，后续更新通过 COALESCE 保留数据库原始小数秒 timestamp，新增真实午夜 backfill→金额修订回归并通过。最终 fresh migration 在另一全新任务数据库执行，未篡改任何已应用 migration checksum。

最终本地验收：5 个新增 PG live 用例通过，既有 8 个 attempt funds/quota PG 用例通过；46 个 memory usage 与 10 个 memory attempt 用例通过；16 个 Gateway daily 用例通过。另在全新数据库运行生产 fresh snapshot→incremental migration smoke，确认新表、backfill 函数及 SQLx 成功记录；逻辑 schema 生成、无漂移检查及新 migration 的表覆盖检查通过。data contracts/PostgreSQL/runtime/Gateway 四个 crate 的 all-features、all-targets Clippy（`-D warnings`）通过，CI shell 语法与 diff whitespace 检查通过。
