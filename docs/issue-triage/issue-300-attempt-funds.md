# #300：同一外部请求的逐 attempt 资金账务

## 范围与状态

本变更实现数据层合同、PostgreSQL 与 memory 适配器、schema、usage 财务写入隔离和 provider 统计重建。一个真实外部 `usage.request_id` 对应一条父记录；每个可能独立收费的上游操作使用不同的 attempt UUID 与 reservation token。

当前为 Draft PR #391：数据层、usage runtime 提交屏障与独立重试、固定规格
同步 JSON 图片公共入口和 hard plan quota 已集成。每日实际费用贡献账本
已整合为 `b22ef5ff8`，主线程在集成树验证完整 data live runner 34 项、
Gateway 专项 21 项（含 17 个真实 PostgreSQL/HTTP 场景）、daily limiter
16 项及自然日/DST 两项全部通过。旧推送 `61f863137` 的四项 required
checks 全绿；新账本集成后的最终检查、Hosted 和完整评审仍须完成。
最新边界与日志见[Gateway 接线](issue-300-gateway-attempt-funds.md)及
[每日账本决策](issue-300-daily-cost-ledger.md)。下文各轮次的数字及待办是
历史验收记录；不能据这些旧状态推断当前入口仍全关，也不能将固定规格
切片等同于 #300/#206 的流式、完整恢复等全部父验收。

## 决定及依据

1. 保留 v1。`attempt_id IS NULL` 的 reservation 继续按 request 唯一；v2 attempt UUID 全局唯一，冻结 provider、provider key、model 与 candidate。v1 entitlement 冲突目标改为同名 partial unique 条件，新账目按 entitlement/request/attempt 独立唯一。新增 migration，同时更新 bootstrap manifest、logical 和 generated schema。
2. 首次 v2 reserve 要求已持久化且未结清的父 usage、完整 owner 身份、没有既有 v1 reservation。资源不足不切换 billing mode。每请求最多 64 个 attempt；重放必须保持 UUID、token、quote、owner 与 provider 一致。
3. `usage.billing_mode=attempt_funds` 是持久财务隔离标识。普通 settle/recover 与 v1 资金 API 拒绝该模式或 v2 token。generic usage、单条 first-byte 和批量 first-byte 只推进客户端生命周期，保留 owner、聚合金额/token 和 billing status；不重复写父 provider 贡献或覆盖 settlement pricing snapshot。
4. `Prepared → Dispatched` 必须先成功持久化再发起上游调用。Unknown 保留完整 hold；NoCharge 释放；Charged 只收 `min(actual, authorized)`。实际费用超过授权时持久保留实际值及差额，进入 reconciliation，不允许 legacy recover 补扣。
5. execution 与 financial outcome 分开：失败/取消也可能收费，成功也可能暂时不知道费用。terminal facts 使用 schema version 1；evidence 必须是最多 16 KiB 的对象，不应存原始 body 或凭据。Unknown 可迟到变为 Charged/NoCharge；已知终态只接受一致 facts 重放。重放投递时间不参与幂等判断。
6. close admission 需要一个同请求已存在 attempt 的完整身份。关闭后不接纳新 attempt，也不允许尚未 dispatch 的工作开始；关闭不释放已 dispatch 的未知费用 hold。Prepared 工作必须明确取消为 NoCharge。
7. `usage_settlement_snapshots.request_funds_summary` 持久区分 known total/actual、collected、held、unknown/prepared 数和 reconciliation。仅关闭 admission、没有未决操作或差额后，父 billing status 才变 settled。浮点金额是读模型投影，整数 summary 与子账是权威证据；单项及聚合 token 受已有 usage INTEGER 存储上限约束。
8. 外部 request/API key/model 请求计数仍来自父行一次；provider 操作计数、成功/错误、费用/token、响应时间和 last-used 来自 dispatch 后的子 attempt。首次切换模式撤销已有父 provider 贡献。provider 主统计及 Codex 窗口 rebuild 都从 legacy 父行与 v2 子事实合并读取，禁止把父聚合全部归给最终 provider。
9. `usage_counter_deltas.request_id` 不是唯一键。每次真实子账转换在同一事务写 outbox，幂等闸门是持久子账状态；即使已处理的 outbox 全被清理，终态重放也不会重复扣款或再入队。provider 月累计沿用既有 actual-cost 语义。
10. PostgreSQL 锁序为 request advisory → usage row → wallet → entitlement（expires_at/created_at/id）→ reservation。钱包锁内不调用通用 usage upsert；财务汇总使用当前事务专用 helper。memory 使用 settlement mutex、共享 usage 父锁和共享 wallet/funds 存储；先验证 staged 汇总，再改财务状态。

逻辑 schema compiler 当前不能表达 CHECK 和 partial index，沿用仓库既有分层：migration/bootstrap 是这些约束的权威，logical 文件记录说明并将 UUID 映射到 PostgreSQL UUID，未扩大修改 compiler。

## API 接入

数据层 `SettlementWriteRepository` 新增：

- `reserve_request_attempt_funds(ReserveRequestAttemptFundsInput)`
- `mark_request_attempt_funds_dispatched(RequestAttemptFundsIdentity)`
- `record_request_attempt_funds_outcome(RecordRequestAttemptFundsOutcomeInput)`
- `read_request_attempt_funds(RequestAttemptFundsIdentity)`
- `close_request_funds_admission(CloseRequestFundsAdmissionInput)`

精确字段以 `contracts/src/repository/settlement/attempt_funding.rs` 为准。memory 构造器需要 `.with_usage_repository(shared_usage)`；未接入父 repository 时 reserve 明确失败。异步事件必须完整保留 UUID/token/owner、version、execution、financial outcome 与 evidence，按存储 quote 定价，不能进入旧 ordinary settle 分支。现有 provider 窗口是按需读取，已改为相同子 attempt 来源；无需新增窗口增量写入。

## 验证记录

在专属 PostgreSQL 17.11 socket-only 实例、独立随机测试 schema、两条不同 backend PID 连接上执行真实迁移和账务测试；未使用生产数据库。Rust 使用 1.95.0，`CARGO_BUILD_JOBS=4`，独占 target。

- schema 子任务：完整 bootstrap 与全部历史 migration 分别通过；真实 CHECK、v1/v2 reservation 唯一性与 entitlement partial conflict 断言通过；schema compose check 通过。
- 新 PostgreSQL live 测试两项通过：`live_attempt_funds_retry_late_charge_and_provider_rebuild_are_idempotent`、`live_attempt_funds_admission_entitlements_and_concurrent_settlement`。
- 加上既有 v1 live 资金测试共 8 项通过：`cargo test -p aether-data-postgres --all-features --lib settlement::funding -- --include-ignored --test-threads=1`，使用隔离 `AETHER_TEST_DATABASE_URL`。
- memory 新增 3 项测试通过，覆盖未知 hold/重试/迟到收费、身份与 token 边界失败无扣款、超过授权封顶与待对账。
- 三个 crate 的全部非 ignored 单元测试通过：`aether-data` 366 passed / 1 ignored，contracts 234 passed，PostgreSQL 231 passed / 36 ignored，合计 831 passed；包含真实临时 PostgreSQL 的完整 startup bootstrap → pending migrations 测试。
- 三个 crate 的 `clippy --all-targets --all-features -- -D warnings` 通过；定向 rustfmt、schema compose check、`git diff --check` 通过。首次 clippy 误用 PATH 中 Homebrew 1.98 的 driver 导致 E0514，改为绝对路径调用 Rust 1.95 的 `cargo-clippy` 后通过，未清除工作区或共享 target。

关键实测：余额 0.20，A 授权 0.08 后 Unknown，B 独立授权 0.08 并收费 0.06，余额 0.14 且仍 hold 0.08；A 迟到收费 0.07 后余额 0.07、hold 0、父记录一条、总实际/已收 0.13。A/B provider 分别计 0.07/0.06、各一次操作，父 API key 计一次外部请求。两连接重复结算同 attempt，以及并发结算同父不同 attempt 均通过。flush、真实 outbox cleanup 后重放没有新扣款或 outbox。单/批 first-byte 与伪造 owner/999 成本的 generic 终态写入未覆盖财务值。主 provider 与 Codex 窗口 rebuild 结果一致。

其他实测包括余额 0.10 时第二次授权只剩 0.02 而被拒绝、无父记录、owner/global UUID/v1 token 冲突、旧 settle/recover/release 拒绝、关闭 admission、同 entitlement 两条 attempt ledger 与 legacy partial conflict 共存、真实 PostgreSQL 超授权封顶和 Prepared NoCharge。

开发中发现并修复：测试 provider key fixture 缺少无默认值的 NOT NULL 字段；真实数据库 `request_metadata` 是 JSON，汇总更新需显式 JSONB 运算再转回 JSON；token 写入需要与 INTEGER 列一致；provider outbox last-used 必须使用 `candidate_last_used_at_unix_secs`，仅写旧 `last_used_at_unix_secs` 不会被当前 provider aggregator 消费。上述问题均重新用 live 用例验证。

完整回归还发现 bootstrap 已含新 DDL，但 privacy/security frontier 后的 migration 仍会执行；因此新 migration 与 bootstrap 同步改为可重入，保留 CHECK 验证，不扩大 snapshot cutoff。同步更新 migration 版本清单和旧 SQL 字面量测试后，完整 startup 与单元测试通过。最终 live suite 使用新的隔离数据库，避免复用开发期 migration checksum。

本代理临时 PostgreSQL 实例验证后已正常停止；数据目录与编译 target 保留供复核，没有清理用户数据。

### 主干集成验证（2026-09-13）

主线程合入 `e730e1247` 后，在自有 socket-only PostgreSQL 17.11 的独立 `aether_attempt_funds_integration` 数据库执行 required live runner，23 个 exact targets 全部实际运行 `1 passed / 0 ignored`，包括原有 v1、#388 retention、两个新 attempt 用例和八个钱包 credit 用例。第一次运行发现 #388 已在共用 fixture 中创建 `usage_http_audits` / `usage_body_blobs`，attempt fixture 再建同名表失败；删除重复建表后完整 23 项通过，未跳过用例或使用 `IF NOT EXISTS` 掩盖隔离错误。ShellCheck 与 diff 检查通过。

## 集成边界

### Gateway/runtime 事件集成开发记录（2026-09-13）

新增独立 typed financial event，携带版本、完整 reservation identity 和可观察的图片输出 evidence；事件不提供可信价格或金额。Gateway writer 从数据库读取冻结 quote 定价。`Outcome` 仅写子 attempt 事实，`ParentLifecycle` 才更新唯一父请求生命周期，两类事件均跳过 ordinary settlement/enrichment。尚未实现从普通 request metadata 或内部报告 body 创建金融 capability 的入口。

对于无法证明未计费的失败、取消或迟到观察，保持 Unknown；当前 `NoCharge` 仅接受尚未 dispatch 的 `prepared_cancelled`。空图片输出不能作为可证明免费或完成收费的证据。已确定收费后到达的旧 Unknown 不覆盖账务状态，避免队列乱序产生永久失败重试。

初版 event 写入接口优先等待队列追加成功。主线程用真实 memory queue 和故障 writer 复现：队列成功会使 API 返回成功，金融 repository 未被调用，不能支持“已持久化”的承诺。改为等待金融 repository 直接提交后才确认观察结果；金融事务自己写入计数 outbox，worker 继续支持历史消息和重复投递，不把异步消费当作提交屏障。已有 `admit_pending_durable` 保留独立文档和父持久化屏障语义。

代理开发阶段 `cargo check -p aether-usage-runtime` 与 `cargo check -p aether-gateway` 通过。主线程新增六项定向回归，初次执行五项通过、一项真实暴露上述队列确认漏洞；修复后六项全部通过，随后 runtime crate 的全部 371 项单元测试通过（零失败、零 ignored）。测试区分 worker/direct 的子 attempt 事实与父生命周期，验证无旧计费/动态价格 enrichment、父写入必须返回行、错误先于父/旧账务写入、Clone/裁剪后的 queue wire 保留完整 capability，拒绝事件提交价格字段和未知版本。

主线程随后执行 Rust 1.95 Clippy，发现内联金融 payload 使所有 usage event 增大并触发 lifecycle seed 的 large-enum-variant。将可选金融 payload 改为 Box，仅金融事件需要该分配，保持既有 JSON wire 结构和 legacy event 的小尺寸；未添加 lint 豁免。runtime 的 `clippy --locked --all-targets -- -D warnings` 已通过。工具链验证显式设置 Rust 1.95 的 `RUSTC`、`RUSTDOC` 及 cargo/cargo-clippy 路径，避免 PATH 中 Homebrew 1.98 覆盖。

Gateway 实际调用计数和新 PostgreSQL 端到端验收仍未完成，不能据编译或 runtime 回归放开收费入口。

主线程继续检查 Gateway drop fallback 时发现第二个实际缺陷：子 Outcome 经 `record_terminal_event_direct` 写入成功后，会取消同一外部请求的父 lifecycle generation。新增回归先真实失败，再将 typed Outcome 的 submit/queue/direct 入口从父终态标记和合并器分离，保留后续 attempt 和父生命周期。

新增 `defer_attempt_funds_event` 复用既有有界 enqueue、direct fallback 和重试 worker，返回明确的 `Persisted`、`Queued` 或 `BufferedForRetry`；后两者不代表金融提交，也不保证磁盘持久性。新增实测在 repository 故障时将同父两个不同 attempt 保留到真实 memory queue，恢复后逐条经 worker 写入且不合并、不走 legacy settlement、不关闭父生命周期；既无可用 queue 又无法提交时明确返回错误。普通 `record_terminal_event_direct` 只尝试数据库写入，不应被描述成 durable queue fallback。修复后全部 373 项 runtime 单元测试、Rust 1.95 Clippy `--all-targets -- -D warnings` 和定向 rustfmt 通过，零失败、零 ignored。

### 报价差异与已知金额的对账语义（2026-09-13）

主线程复审发现 Gateway 将 `BillingImageQuotedCalculation.requires_reconciliation` 丢弃。该标记还覆盖输出格式、尺寸或质量偏离冻结报价；既有 `output_format_changes_are_audited_even_when_the_cost_does_not_change` 回归证明 `excess_units == 0` 仍可能需要对账。仅按实际金额超授权判断，会把已知收费但输出不符的请求错误标为 settled。

决定正式贯通合同：`RequestAttemptBilledUsage` 增加 `requires_reconciliation`，旧 JSON 缺省为 false。memory 和 PostgreSQL 都通过同一 `reconciliation_facts` 计算保留报价差异及金额超限原因；父汇总同时保留子事实标记。已知实际金额仍按授权封顶收取，结算后的 hold 为零，但 reservation 保持 ReconciliationPending、父 billing status 保持 pending，不能把已知金额退化成 Unknown 或直接消除审计要求。已有终态只接受一致重放，不能通过后续事件清除该标记；人工对账处置仍需独立、可审计的工作流。

新增 memory 与真实 PostgreSQL 回归：授权 .08、实际 .06 且报价需对账，确认实际与已收 .06、差额 0、hold 0、父 pending；一致重放不双扣，清除对账标记的冲突重放被拒绝。PostgreSQL 用例进一步在真实 outbox flush/cleanup 后重放，余额仍 .14 且没有新增计数投递。已登记到 `tools/ci/run_postgres_live_tests.sh`，runner 增至 24 个 exact targets。

本轮本地验证使用 Rust 1.95.0 和专属 socket-only PostgreSQL 17.11：

- `cargo test --locked -p aether-data --all-features repository::settlement::memory:: --lib`：24 passed，0 failed / 0 ignored。
- `cargo test --locked -p aether-data-postgres --all-features settlement::funding::attempts::tests:: --lib -- --include-ignored --test-threads=1 --nocapture`：3 passed，0 failed / 0 ignored，包括两个既有并发、重试、迟到收费和 provider 重建用例。
- 三个 data crate 的 `clippy --locked --all-features --all-targets -- -D warnings` 通过，定向 rustfmt 与 diff 检查通过。

工作树已快进整合远端 `9862905bf` 与 main `4a74b11ef`，保留全部未提交 Gateway 改动。Gateway 代理继续处理 reserve 提交时取消、异步作用域和实际入口等最终评审项；本节数据验证不代表该部分已经验收，也不将 #391 转为 Ready。

### 财务先完成时仍允许客户端生命周期收尾（2026-09-13）

Gateway reserve/cancel 与 close 后父写入重试两项真实 PostgreSQL 回归暴露同一缺陷：子 reservation 已释放或收费、admission 已关闭、父 billing status 已 settled，但父 status 仍停在 pending/streaming。通用 upsert 返回了行，SQL 的 `billing_status = 'pending'` 条件却阻止了终态字段更新。延长等待或提前写父终态都不能修复生命周期与财务状态互相独立的约束。

仅对服务器标记的 `billing_mode = 'attempt_funds'` 放开客户端状态、时间、最终路由与 capture 更新；继续先执行已有生命周期 revision gate，旧终态及终态后的非终态事件是完整事务 no-op。owner、billing status、费用和 token 仍取冻结父汇总，金融 setter 未放宽，provider 费用仍由子 attempt 独立计数。legacy settled 请求保持原有不可覆盖规则，无 schema 或公共 API 改动。

新增真实 PostgreSQL target `live_attempt_funds_settled_parent_accepts_final_lifecycle_without_financial_mutation`，覆盖 Prepared NoCharge 与 dispatched 已收费两种先结账路径，随后 cancelled/completed 终态、时间、最终路由和 body 能保存；伪造 owner/API key/金额/token/standalone metadata 均不覆盖冻结事实，旧 terminal 与新 nonterminal 重放不修改存储。小 body 按现有存储策略转为 Reference，读取时还原原内容。四项 attempt live tests 全通过（0 ignored），113 项 usage 单元测试通过（12 个既有 live targets 在该单元调用中 ignored），PostgreSQL 全特性全目标 Clippy `-D warnings` 通过。required data runner 增至 25 个 exact targets；完整 runner 与 Gateway 回归结果后续补充。

完整 required data runner 随后在同一专属 PostgreSQL 17.11 上通过，25 个 exact targets 每项均为 `1 passed / 0 failed / 0 ignored`，包含迁移、legacy 生命周期 no-op、并发 quota、既有资金/retention、四项 attempts 和八项 credit。定向 rustfmt、Bash 语法、ShellCheck 和 diff 检查通过。该数据层修复不会更新 Gateway 的最终验收状态。

前一已推送 head `4a9b08673` 的四项 required checks 均已 SUCCESS，Rust CI run `34749410358` 和 CodeQL run `34749406815` 完成；Data DB Live job `103703097140` 已逐项核验原 24 个 target 各 `1 passed / 0 ignored`。这些 hosted 结果不代表本节新修复或未提交 Gateway 已验收。main `4a74b11ef` 的 CodeQL run `34748724974` 仍 queued/no jobs，不能以 PR 扫描替代。

### Gateway 实测接入 required CI（2026-09-13，待最终集成）

新增 `run_gateway_attempt_funds_live_tests.sh` 串行执行八个明确的 Gateway PostgreSQL/真实 HTTP 用例。沿用 `Test (Gateway)` 的构建、Rust 栈和 required 汇总规则，仅为该 job 增加一次性 PostgreSQL 16 service 与执行步骤；避免在 data job 重建整个 Gateway。脚本必须提供专属数据库 URL，每项 fixture 在真实迁移后的唯一 schema 中隔离；不会读取开发者默认连接。非 ignored 的 memory、报价投影和入口门禁测试仍由既有 nextest 执行。

八个 target 已与当前测试声明逐一核对；Bash 语法、ShellCheck、actionlint 和 12 项既有 Rust CI 选择/固定引用测试通过。首次 Node 验证缺少 `js-yaml`，按已有 lockfile 安装 automation 依赖后通过，未修改依赖文件。此处只记录 CI 接线和静态验收，真实八项运行及完整 Gateway 审查结果另行补充；尚未提交的 CI 变更不能视为 hosted 验收。

### 尚未完成的接线

- 本文不代表 Gateway 的“资金 dispatch 持久化后才能调用上游”、请求结束 close admission、可靠结果写入或父持久化失败处理已经通过验收；正在实施的接线须通过真实 Gateway 与 PostgreSQL 测试后才能交付。实现状态另见 `issue-300-gateway-attempt-funds.md`。
- 父 summary 已存储且 close API 可返回；用户界面/通用 read model 的 summary 展示不在本数据层范围。
- memory 的既有钱包适配器没有 PostgreSQL entitlement 数据源；真实 entitlement 分配、并发锁与 ledger 验收以 PostgreSQL 为准。
- #388 retention 已随主干合入，23 项 live 回归通过；该结果验证数据层集成，不替代 Gateway/runtime 的完整生命周期验收。
