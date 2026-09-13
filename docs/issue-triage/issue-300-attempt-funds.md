# #300：同一外部请求的逐 attempt 资金账务

## 范围与状态

本变更实现数据层合同、PostgreSQL 与 memory 适配器、schema、usage 财务写入隔离和 provider 统计重建。一个真实外部 `usage.request_id` 对应一条父记录；每个可能独立收费的上游操作使用不同的 attempt UUID 与 reservation token。

数据层阶段已提交并推送至 Draft PR #391，集成提交 `73222d17cba56089609bb62266732cffdff1c2c4` 包含 #388 retention、#390 钱包 live CI 及 #367 主干变更。两个新 attempt live 用例已登记到 required runner（共 23 个 exact targets）。usage runtime typed event、金融提交屏障和重试隔离已在本地实现并通过 373 项单元测试及 Clippy；Gateway 执行接线和应用构造器共享 memory usage repository 仍属于进行中的集成验收，收费图片入口尚未启用。

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

### 尚未完成的接线

- 本文不代表 Gateway 的“资金 dispatch 持久化后才能调用上游”、请求结束 close admission、可靠结果写入或父持久化失败处理已经通过验收；正在实施的接线须通过真实 Gateway 与 PostgreSQL 测试后才能交付。实现状态另见 `issue-300-gateway-attempt-funds.md`。
- 父 summary 已存储且 close API 可返回；用户界面/通用 read model 的 summary 展示不在本数据层范围。
- memory 的既有钱包适配器没有 PostgreSQL entitlement 数据源；真实 entitlement 分配、并发锁与 ledger 验收以 PostgreSQL 为准。
- #388 retention 已随主干合入，23 项 live 回归通过；该结果验证数据层集成，不替代 Gateway/runtime 的完整生命周期验收。
