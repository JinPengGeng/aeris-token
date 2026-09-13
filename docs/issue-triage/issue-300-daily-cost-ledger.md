# Issue 300：每日实际费用持久计数决策

状态：实现已集成到 PR #391 分支（`b22ef5ff8`），独立代码评审接受，
集成树的完整 PostgreSQL live runner **34 个 exact targets 全部通过**，
每项 1 passed / 0 failed / 0 ignored。Gateway **21 项专项全部通过**，
包括 17 项真实 PostgreSQL/HTTP 场景。集成树的 16 项 daily limiter、两项
自然日/DST 和四个 crate 严格 Clippy 均通过；新提交 Hosted CI 及最终
完整评审仍须完成，本状态不表示 PR 已合并或父 #300 已完成。

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

## 实施及已取得的验收证据

运行与升级细节见[每日账本操作说明](../operations/daily-cost-ledger.md)。
贡献与普通 usage 或 attempt 财务事实同事务提交；pending/first-byte 批量
路径按批冻结身份，没有逐行加载完整审计。前门一次 SQL 读取两个作用域，
不再调用 Redis 累加或恢复 hook。

主线程在全新任务数据库 `daily_ledger_integrated_391` 运行生产 bootstrap
与全部增量迁移，再运行完整 `tools/ci/run_postgres_live_tests.sh`，34 项
全部通过。五项新账本测试覆盖迟到原日、重放、事务回滚、归属冻结、批量
identity、真实 backfill、清理保留及午夜小数秒回填后金额修订；其余 29 项
既有 usage、funding、quota、session 和 wallet credit 测试同时通过。
日志：`/private/tmp/aeris-391-daily-integrated-data.log`。

实现分支先前通过 46 项 memory usage、10 项 memory attempt、16 项
Gateway daily 测试和四个 crate 严格 Clippy。主线程已读取实现与交接，
独立评审接受集成提交的事务、整数金额、冻结身份/时间、backfill 和启动
边界。主线程随后在集成树运行全部 21 项 funded Gateway 专项，0 failed /
0 ignored；新公共 HTTP 测试验证普通和 standalone 的 `.06 + late .07`
只计 `.13`，下一请求因 `.10` 每日额度拒绝、零新预留、零额外上游调用。
日志：`/private/tmp/aeris-391-daily-integrated-gateway-fixed.log`。首次编译因
新测试漏导入读取 trait 而失败，补齐 import 后才获得上述行为验收。

集成树随后通过 16 项 daily limiter 和两项自然日/DST 测试，以及 data
contracts/PostgreSQL/runtime/Gateway 四个 crate 的 all-features/all-targets
Clippy（`-D warnings`，3m27s）。全 workspace rustfmt 修正测试子模块排序
后通过；Gateway 环境参考漂移及五项 Python 测试、schema 生成内容/新表
覆盖检查和两个 live runner ShellCheck 通过。

已有 APP_TIMEZONE 自然日函数处理 DST；账本查询采用 `[start,end)`。
10 万条合成记录的 scoped SQL EXPLAIN 约 6.884 ms，不代表生产 P99。
首次部署必须排空旧 writer 后迁移，再启动新读写；不支持任意混版本
滚动切换，升级前已丢失唯一财务来源的历史不能凭空恢复。

评审另确认生成 SQL 文件保留了现有生成器输出的末尾空行，完整提交范围
的 `git diff --check` 会报告该格式提示；不手改生成产物或扩大本次资金
实现去改全局生成器。其余手写变更的 whitespace 检查正常，schema 漂移
检查作为独立必验项保留。先前交接的“diff 全通过”不适用于这个完整范围。
