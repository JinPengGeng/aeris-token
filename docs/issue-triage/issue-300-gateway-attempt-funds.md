# #300 Gateway 逐 attempt 资金接线

状态：固定规格同步图片公共入口已接入每 attempt 资金与 hard plan quota 的联合准入。当前源码 **18 项专项通过、0 失败、0 ignored**，包括 14 项真实 PostgreSQL/HTTP 场景；另有 65 项本轮定向回归通过。最终全特性/全目标 Clippy（`-D warnings`）通过，耗时 3m05s；新 HEAD 的 Hosted 检查仍须在推送后确认。数据层独立 29 项 PostgreSQL runner 已通过，不能替代 Gateway 的 Hosted 验收。Draft #391 和父 #300 保持开放，前门每日实际费用计数与最终完整集成评审尚未完成。下文保留旧轮次的实测记录，其阶段性限制以本节最新状态为准。

## Hosted CI 测试目录修正

推送 `e0cf5c242` 后，Rust run `34752956043` 的 Shell security fixtures
在环境参考完整投影检查失败。复现确认唯一差异是测试专用的
`AETHER_TEST_DATABASE_URL`：新测试平铺在生产模块旁，未命中现有生成器的
`tests` 目录排除规则。测试现移动到 `funded_image/tests/`，保持 Rust 模块名和
CI exact targets 不变；未改生产环境参考或扩大生成器排除逻辑。

移动后，环境参考的五项 Python 测试、生成内容漂移检查、`git diff --check`
以及 `cargo check -p aether-gateway --tests` 均通过。原 140 项行为测试未因
纯目录移动重复执行；新 HEAD 的 Hosted checks 仍须单独核验。

## 公共入口接线修订

### Hard plan quota 接入（本地行为验收通过）

已将独立数据层提交 `acf916f39` 集成到当前分支为 `2eff2e037`。
公共 HTTP 的 `PlanUsageReservationContext` 从可信 request extensions 进入
同步请求作用域，并由候选循环及 heartbeat 后台工作继承；policy 的 subject、
token、原 admission 时间和窗口不从 report metadata 获取。metadata 中的
token 仅用于一致性检查，缺少可信上下文或不匹配时拒绝。

已授权的付费图片跳过 legacy 单父 cost reservation，prepare 使用冻结的
policy 与每次独立报价在同一数据库事务中预留 quota 和资金。配额不足沿用
`PlanUsageLimited`；standalone 不继承创建用户的 hard plan，unlimited
钱包和 entitlement 不绕过用户 hard plan。首个 reserve 被明确拒绝时也
持久写入零费用失败父终态，避免 durable pending 遗留；已有 attempt 的拒绝
使用此前持久身份更新父展示状态，不覆盖 Unknown hold 或实际费用。

新增三个真实公共 HTTP/PostgreSQL 测试：现金均为 `.20` 时对比 `.10/.20`
配额重试；Unknown、`.06 + .07 = .13` 迟到收费与清理后重放；四类账户
配额边界；并发不同请求共享 `.10` 配额仅发送一次。首次诊断执行这三个
新测试全部通过。随后使用原有 16 MiB 测试栈重新执行全部 18 项专项，
全部通过且零 ignored；原 policy 10、candidate loop 34、sync execution 21
项回归也全部通过，共 83 项。本轮日志为
`/private/tmp/aeris-gateway-hard-quota-final-20260913.log` 及同日
`aeris-quota-policy/candidate/sync-regressions` 日志。新的三个 live exact targets
已加入 required Gateway runner，Hosted 结果单独跟进。

首次整组执行暴露新增 `request_scope` 转发层使调试 future 栈溢出；已移除
这个额外 async 层，沿用原有嵌套作用域路径，未上调 CI 的测试栈配置。
32 MiB 仅用于已有产物的三个公共新场景诊断，不作为最终验收。

Hosted 原 run `34752956043` 的 Gateway job `103712483998` 实际通过前三个
live targets，在取消场景因 `terminal_facts` 为 NULL 失败。原因是
`unknown_attempts` 在 dispatched、尚未写取消事实时已经为 1，原测试
过早结束等待。现等待真实 `terminal_facts.execution.status=cancelled`
与 admission closed 同时可见，保留后续严格状态断言。该修正未修改生产
取消语义。环境文档修复 `cc62835c3` 的 Hosted shell fixtures 已通过。

前门每日实际费用计数仍未完成，后续方案与验收见
[每日费用账本决策](issue-300-daily-cost-ledger.md)。

### 主线程实测修正

真实公共 HTTP 首先暴露两处 fixture 问题：服务端缺少生产路由所需的 `ConnectInfo<SocketAddr>`；`custom` provider 按现有能力表只允许一张输出，而正例需要授权八张。测试现沿用生产的 `into_make_service_with_connect_info`，并使用明确支持该数量的 `openai` provider。没有修改生产路由依赖或图片数量上限。

随后实测发现真实 standalone 准入缺陷：Gateway 将资金 identity 的 `user_id` 清空，但管理员创建的 standalone key 仍属于创建用户，PostgreSQL 会验证这一归属，因而返回 owner conflict。现父 usage 和资金 identity 均保留认证用户；standalone 标志及倍率由认证上下文写入。资金来源仍由 standalone 标志限制在 key wallet，不回退到用户钱包或 entitlement。公共正例额外创建余额 .30 的用户钱包，确认它保持不变，只有 key wallet 从 .20 变为 .14，父归属与 standalone 标志正确。

unlimited 的初版断言错误地要求现金余额变为 -.06。核对现有普通 settlement、funding 实现及真实数据库后，沿用已有语义：现金余额保持 0，`total_consumed` 和 postpaid allocation 的已记账金额均为 .06。测试同时断言这两项持久结果，不将 unchanged cash 解释为免费请求。无钱包 entitlement 正例亦已通过。

最终统一命令为 `cargo test -p aether-gateway --lib execution_runtime::funded_image::tests:: -- --include-ignored --test-threads=1`，Rust 1.95、既有独立 PostgreSQL 17.11、`RUST_MIN_STACK=16777216`；结果 15 passed / 0 failed / 0 ignored。required Gateway CI 已接入独立 PostgreSQL service 和 11 个 exact live targets；Hosted 实测将在源码推送后确认。

同一编译产物的定向回归另有 125 项通过：auth gate 30、同步图片转换 4、流式图片转换 3、钱包 auth 12、同步执行 21、候选循环 34、orchestration 21。钱包 auth 增加 inactive wallet 配合正数 entitlement 仍拒绝的反例，防止无钱包放行扩大为禁用钱包放行。日志为 `/private/tmp/aeris-gateway-public-regressions-20260913.log`。`actionlint .github/workflows/rust-ci.yml`、Gateway runner ShellCheck 和 diff 检查均通过。

主线程在相同 Rust 1.95 环境执行 `cargo clippy -p aether-gateway --all-features --all-targets -- -D warnings`，2m49s 成功；Gateway rustfmt 通过。随后仅同步 main 的 #394 两份单维护者文档，没有运行时代码冲突。上述本地验收覆盖固定规格 JSON 的资金切片，并不证明尚未实现的 hard-policy、每日计数或供应商真实收费回执。

`execution_plan_balance_capacity_rejection_inner` 在 standalone/unlimited/无有限钱包容量早退之前调用图片准入。免费必须有明确 free-tier 计价；付费必须能从最终 provider JSON 投影生成确定授权上限，并处于服务端同步请求作用域，具备 usage/settlement 持久化能力。未知计价、缺失价格、付费流式和多阶段投影返回明确 422；不再由账户类型绕过此检查。同步 heartbeat 若已发送 HTTP 200 响应头，则通过既有 JSON error 体报告准入拒绝。

公共 gate 将完整报价保存在服务器请求作用域，以最终执行计划（去掉候选记账 ID）、认证用户/key/独立 key 标志/倍率和模型/adapter 上下文的 SHA256 绑定；不输出或存储明文认证头作为索引。prepare 消费同一冻结报价，候选重试仍获得独立 attempt/token。真实资金是否足够由数据库原子 reserve 决定。gate 后才附加的旧 hard-cost policy token 也不能混入冻结报价。

新增真实公共 router HTTP 测试覆盖普通有限钱包、standalone key、unlimited postpaid、无钱包 entitlement。正例要求六张输出、.06 实际收费、零 hold、单父 completed/settled；负例对每类账户发送 auto-size 和付费 stream，要求明确错误、零 reservation 和零上游 HTTP。测试使用真实 PG usage/wallet/entitlement/settlement、内存固定模型价格和真实本地上游，不使用 execution override。

无钱包 entitlement 在更早的 auth 阶段还存在独立阻塞：旧逻辑只让剩余额度修复 BalanceDenied。现同时允许不存在钱包但具有正数 active grant 的普通用户；已有 inactive wallet 不受此放行条件影响。真实 grant 容量仍由 reserve 事务检查。

旧图片转换/stream 回归原本没有 billing reader，现通过明确 test-only free-tier fixture 保持其传输测试语义；生产缺失价格仍拒绝。生产 heartbeat 测试改为 `production_image_heartbeat_executes_funded_public_attempt`，验收六张结果与一次实际上游调用。

新增 ignored exact targets（前缀 `execution_runtime::funded_image::tests::public_tests::`）：

- `live_public_images_fund_user_standalone_unlimited_and_no_wallet_entitlement`
- `live_public_images_reject_unbounded_and_stream_before_every_account_shortcut`
- `live_public_image_retry_reserves_each_send_and_retains_unknown_hold`：真实公共请求候选重试；余额 .10 只发送首次 .08，余额 .20 可独立授权第二次并记 .06；两者均保留首次 Unknown hold，单父、多独立 attempt/token。

每日硬额度仍是未完成项。合同提案记录在 `/private/tmp/aeris-attempt-daily-quota-contract-proposal.md`：必须与资金在同一事务占用 `known actual + held + 新授权`，Unknown 不可由旧 TTL/父失败分支释放；主线程还在协调独立 attempt quota 审查意见。本轮没有删除旧 token 拒绝或修改数据字段，不能把此拒绝算作每日硬额度集成完成。

## 集成复审修订

逐项回应 `/private/tmp/aeris-gateway-inflight-review.md`：

1. close 和父终态写失败不会丢弃暂存事件，正常完成后重试不会误标取消。新增 PostgreSQL sequence/trigger 故障注入，只让第一次 close 或父终态 UPDATE 失败，检验真正的 drop 重试。整个请求的父终态时间在 admission close 成功后才冻结，避免使用 reserve 前的兜底时间。
2. 同步资金作用域上移至 `run_ai_sync_execution_path` 外层，跨候选来源复用。两处图片 heartbeat 后台任务在 spawn 前取得共享请求持有权，并显式安装 task local；最后一个持有者才执行 close。客户端断开沿用现有 cancel_on_disconnect 策略。PG advisory lock 测试将 B reserve 阻塞，验证 A Unknown 后可以先返回 heartbeat 响应头、admission 仍开启，解锁后 B 真正发送并最终 close。早期生产 heartbeat 拒绝回归已由顶部公共入口修订替换为资金执行成功回归。
3. 从 durable pending 到 reserve 结果交接改为受 runtime producer 跟踪的独立任务，客户端取消只关闭接收端。该任务继续取得 reserve 提交结果；若接收端已关闭，明确取消 Prepared。close 等待准备任务结束，避免先关闭 admission、后出现已提交 reservation。新增测试在真实 PostgreSQL reserve INSERT 内等待 advisory lock，取消调用任务后才允许事务提交，验证余额与 hold、父取消及零 HTTP 调用。该测试不等同于进程崩溃后的完整恢复验收。
4. 付费范围进一步限定为无 proxy、无 transport_profile、无原始 base64 body 或 content_encoding 的直接 JSON 请求。内部控制头、流式、多阶段 adapter 均拒绝；旧 OAuth resend 禁用。真实 funded task 选择独立 reqwest cache key，并显式配置 `retry::never()`，默认不跟随重定向；候选重试重新授权。
5. 正常 .20 验收改为固定 `1024x1024`、`high`、png、每张 .01，授权 `n=8`、实际 B 6 张和迟到 A 7 张。不会通过改变授权 quality 来凑 .06/.07。Gateway 将 `priced.requires_reconciliation` 写入主线程新增的数据字段；另测 quality 或 output_format 改变但金额不超额时仍要求对账。
6. 模拟上游一直监听，额外 GET barrier 经过同一 accept/read 循环后才断言发送次数，避免测试在已发送请求被服务端观察前结束。
7. 旧 hard-cost quota reservation 是单父、不可变终态合同，无法表达多 attempt 的 Unknown 和迟到收费。目前付费图片旧 cost estimate 为未知，会在 reserve 之前拒绝。Gateway prepare 另外显式拒绝被候选循环附加 `plan_usage_reservation_token` 的付费请求，沿用候选错误分支释放旧 reservation，避免成功 v2 outcome 跳过旧 quota reconciliation。新增拒绝前无父 usage/资金 admission/HTTP 的回归；旧配额错误释放回归仍通过。此阶段不支持混合 hard-cost quota 与付费图片 attempts。
8. Gateway 正常 backend 工厂只配置 PostgreSQL，disabled backend 没有 usage/settlement writer，并不隐式建立两个 memory store。新增 memory Gateway 验收显式将同一 usage Arc 同时交给 Gateway writer 和 `InMemorySettlementRepository::with_usage_repository`，验证一个父记录同时呈现 completed、settled 和 .06 账务金额。

复审第一轮 PostgreSQL 实测阻塞：`usage/queries/upsert_sql.sql` 将 status/finalized_at 等更新限定为旧 `billing_status='pending'`。v2 在 close 后已变为 settled，typed ParentLifecycle 写入因此返回旧行却不更新客户端状态。两个失败分别保留在 `gateway_attempt_5e20c9268ddd4b38b5de8e9be6c4512d`（streaming/settled/closed）和 `gateway_attempt_3cca0501d1734c069f7a977e5e7c2969`（pending/settled/closed，Prepared 已释放）。数据修复属于主线程写集，不能通过提前写父终态或放宽断言绕过。

drop 直接写失败后现调用 `defer_attempt_funds_event`。日志分别报告 `Persisted`（确认提交）、`Queued`（队列接受）、`BufferedForRetry`（本地重试缓冲）和失败。后两者只表示保留重试工作，不宣称金融提交或磁盘持久性。

复审专项 exact targets 均以 `execution_runtime::funded_image::tests::` 为前缀；原 5 项及新增如下 7 项包含在同一测试命令中：

- `live_gateway_image_attempt_reserve_commit_after_cancellation_is_released`
- `live_gateway_image_attempt_close_and_parent_write_retry_preserve_terminal`
- `live_gateway_image_attempt_changed_specs_require_reconciliation_without_overrun`
- `live_gateway_image_heartbeat_keeps_admission_after_response_headers`
- `memory_gateway_image_attempt_uses_shared_parent_usage_repository`
- `paid_image_rejects_legacy_hard_quota_before_funds_admission`
- `production_image_heartbeat_executes_funded_public_attempt`（替换早期 gate-closed 回归）

复审日志：`/tmp/aeris-gateway-funds-revision-tests.log`（12 项专项第一轮）；`/tmp/aeris-gateway-funds-revision-sync.log`（21 通过）；`/tmp/aeris-gateway-funds-revision-candidates.log`（34 通过）；`/tmp/aeris-gateway-funds-revision-orchestration.log`（21 通过）；`/tmp/aeris-gateway-funds-revision-clippy.log`（全特性/全目标通过，2m58s）。原有 76 项回归使用第一轮新增测试的编译产物；最终 transport retry/fallback timestamp 修改已由 Clippy 检查，重新运行专项由主线程当前 exact harness 接手。本代理 Cargo sessions 89609/70644 均结束，不与主线程 session 51058 并发 cargo。

本轮开始时 PostgreSQL 已由其他工作重新启动（PID 25584）；本代理复用唯一 schema，不停止其他持有者正在使用的实例。原目录和失败 schema 保留供复核。

### 最新入口边界核查

前轮普通有限钱包的拒绝回归不能扩大解释为所有账户类型：旧 `execution_plan_balance_capacity_rejection_inner` 对 standalone 和 unlimited 的早退发生在未知图片报价拒绝之前。该发现触发本节上方“公共入口接线修订”，当前源码已将图片准入移到早退前；新增四类账户公共 HTTP 测试已统一通过。旧回归名称仅代表前轮状态，不是当前能力声明。`executor/remote.rs` 的两处远端执行判定当前均直接返回 `Ok(None)`，不是实际存在的付费远端执行入口；不能凭接口名称声称远端收费已接线。

### 复审精确命令

工作目录 `/private/tmp/aeris-attempt-funds.U3xxiO`。专项测试实际命令等同于本文末尾 Rust 环境命令，追加日志重定向：

```sh
rtk proxy env \
  PATH='/Users/fengying/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin' \
  RUSTC=/Users/fengying/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin/rustc \
  RUSTDOC=/Users/fengying/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin/rustdoc \
  CARGO_TARGET_DIR=/tmp/aeris-attempt-funds-target.LLh9Z1 \
  CARGO_BUILD_JOBS=4 RUST_MIN_STACK=16777216 \
  AETHER_TEST_DATABASE_URL='postgresql://fengying@localhost/postgres?host=/tmp/aeris-gateway-funds-db.Mh5Mzm&port=57419' \
  /Users/fengying/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin/cargo test \
  -p aether-gateway --lib execution_runtime::funded_image::tests -- \
  --include-ignored --test-threads=1 --nocapture \
  > /tmp/aeris-gateway-funds-revision-tests.log 2>&1

rtk proxy env \
  PATH='/Users/fengying/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin' \
  RUSTC=/Users/fengying/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin/rustc \
  RUSTDOC=/Users/fengying/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin/rustdoc \
  CARGO_TARGET_DIR=/tmp/aeris-attempt-funds-target.LLh9Z1 CARGO_BUILD_JOBS=4 \
  /Users/fengying/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin/cargo-clippy clippy \
  -p aether-gateway --all-features --all-targets -- -D warnings \
  > /tmp/aeris-gateway-funds-revision-clippy.log 2>&1

rtk proxy env RUST_MIN_STACK=16777216 /tmp/aeris-attempt-funds-target.LLh9Z1/debug/deps/aether_gateway-267520d3166a6fde execution_runtime::sync::execution::tests --test-threads=1 > /tmp/aeris-gateway-funds-revision-sync.log 2>&1
rtk proxy env RUST_MIN_STACK=16777216 /tmp/aeris-attempt-funds-target.LLh9Z1/debug/deps/aether_gateway-267520d3166a6fde executor::candidate_loop::tests --test-threads=1 > /tmp/aeris-gateway-funds-revision-candidates.log 2>&1
rtk proxy env RUST_MIN_STACK=16777216 /tmp/aeris-attempt-funds-target.LLh9Z1/debug/deps/aether_gateway-267520d3166a6fde executor::orchestration::tests --test-threads=1 > /tmp/aeris-gateway-funds-revision-orchestration.log 2>&1
```

## 已采用的接线

采用服务器内的请求作用域和 attempt 作用域，能力不经过 report metadata。同步候选循环持有一个请求 admission 作用域，各真实付费 operation 在最终执行前重新生成 UUID/token，依次等待父 usage 写库、reserve 和 dispatch 提交。单次取消以数据库中的 dispatch 事实决定 Prepared cancellation 或 Unknown；已发送的失败/取消不能当作免费。最终父生命周期与子资金 outcome 分离，候选循环收尾关闭 admission，Unknown hold 不释放。

首个可证明的范围限定为最终 provider JSON 请求的同步 OpenAI images generations/edits、明确尺寸/质量/数量、无 preview。沿用已有 BillingImageAuthorizationQuote 计算器，冻结价格和 provider/model/key。没有服务端 token 上限证明的 token 计价、auto/default 维度、SSE、Codex/ChatGPT-Web/Grok 等多阶段路径继续拒绝。付费路径禁止旧 OAuth 内部重试复用同一 reservation；失败交回候选循环，每次候选调用独立授权。

请求 scope 围住完整候选循环，attempt scope 只围住当前 operation。scope 中的 capability 不来自任何外部请求、内部报告 JSON 或通用 metadata，内部报告不能伪造收费金额或使用普通 lifecycle 分支结算 v2 reservation。

补充边界：最终模型名称必须匹配选中 billing model 的 provider model；暂不接受 processing tier 变化或内部 transport 控制头。图片 evidence 只含完整输出数量、响应明确声明的 size/quality/format 和 token 观察，不含图片内容或可信金额；响应没有完整 evidence 时写 Unknown。HTTP failed/cancelled 与收费证据分开，迟到收费必须复用数据库里已经冻结的执行事实，不能改写第一次观察的状态或耗时。

取消或 dispatch 返回错误时读取数据库中的 `dispatched_at`，不依据本地布尔值释放资金。准备完成至 terminal guard 建立之间发生取消也有独立 drop 恢复；观察到的 evidence 直接写库失败时保留原事件重试，并在 drop 恢复失败时交给已有 runtime 投递路径。该投递不作为账务提交成功的确认。正常结束显式等待 close；close 失败不提前丢弃待写父终态。新 attempt reserve 成功后清除上一候选暂存的父终态，避免后续取消把上一候选失败重放成客户端终态。

重试路径可能没有正常 terminal usage payload，因此在成功 reserve 时同时创建服务器内父生命周期兜底事件。候选重试耗尽或下一候选授权失败后，父请求写 Failed；请求作用域取消则写 Cancelled/499。该父状态不改变子 attempt 的 Unknown 金融状态。正常 future 已返回而 close 失败时，drop 重试保留正常结束语义，不误改成取消。

## 初轮接线验收记录（复审前）

验证环境使用 Rust 1.95.0、PostgreSQL 17.11、socket-only 临时实例 `/tmp/aeris-gateway-funds-db.Mh5Mzm`（端口 57419），复用本任务独占 target `/tmp/aeris-attempt-funds-target.LLh9Z1`。Gateway 测试沿用 required CI 的 `RUST_MIN_STACK=16777216`；默认测试线程栈会溢出，不能省掉该现有设置。数据库 fixture 执行真实迁移后，在唯一 schema 中复制所需表和 usage view；LIKE 保留索引/检查，但不复制生产外键。本轮验证 Gateway 执行屏障、实际 HTTP 调用和账务对账，数据外键/并发事务完整性沿用数据层单独验收。

最终五项专项测试全部通过（四项真实 HTTP/live PostgreSQL，一项投影和 evidence 边界单测），并以最终测试产物再次通过 21 项原同步执行测试和 34 项候选循环测试，共 60 项通过。真实 HTTP 测试使用 Gateway direct transport，没有 execution override；定价读取由内存 fixture 控制，usage、wallet、reservation、outbox 都落 PostgreSQL。

以下 target 都位于 `execution_runtime::funded_image::tests::`：

| Exact target 后缀 | 验收事实 |
| --- | --- |
| `live_gateway_image_attempts_retry_late_charge_and_replay` | 余额 .20；A 授权 .08 后 Unknown，真实 Retry 到 B；B 授权 .08、实际 .06，A 迟到 .07；两次 HTTP、单父请求、合计 .13、余额 .07；provider 分别一次 .07/.06，API key 外部请求只计一次；processed outbox 清理后重放不双扣，父 completed/settled。 |
| `live_gateway_image_attempts_reject_second_upstream_when_held` | 余额 .10；A 持有 .08 后真实 Retry，B reserve 被拒；HTTP 总数严格为 1，保留 .08 hold，admission closed，父 failed。 |
| `live_gateway_image_attempts_persistence_failure_sends_no_upstream` | PostgreSQL trigger 分别注入父 INSERT、dispatch UPDATE 失败；实际 HTTP 均为 0；dispatch 回滚后 Prepared 明确取消，hold/prepared 均为 0。 |
| `live_gateway_image_attempt_cancellation_preserves_dispatched_hold` | 本地上游收到真实请求但不回复时取消 Gateway task；子 Unknown、admission closed、父 cancelled；.08 hold 保留，余额仍 .20。 |
| `image_quote_rejects_unproven_projections_and_evidence` | 不可证明的计价投影和缺失/越界 evidence 保持拒绝或 Unknown。 |

专项测试初次强化 Retry 断言时暴露 fixture 缺少 planner 的 `candidate_index`；补齐各候选真实索引及 `retry_index` 后，保留 Retry 断言并通过全部场景。模拟上游在计划响应耗尽后继续监听，以便未经授权的额外发送确实能增加调用数；不会用已关闭监听器掩盖错误发送。

迟到收费验收使用 runtime typed financial event 和 Gateway writer 进入已有数据事务。本轮没有新增公开 provider webhook 或 receipt API，也未验证真实外部供应商的收费回执。

Clippy `-p aether-gateway --all-features --all-targets -- -D warnings` 通过。检查发现已有 `handlers/admin/billing/mod.rs` 把 tests 模块放在生产 handler 之前，触发 `items_after_test_module`；本轮仅将同一测试模块移动至文件末尾，没有改业务逻辑或压制 lint。

可复核的专项命令如下（`PATH`、`RUSTC`、`RUSTDOC` 同时固定，避免 cargo-clippy 子进程误用 Homebrew cargo）：

```sh
rtk proxy env \
  PATH='/Users/fengying/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin' \
  RUSTC=/Users/fengying/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin/rustc \
  RUSTDOC=/Users/fengying/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin/rustdoc \
  CARGO_TARGET_DIR=/tmp/aeris-attempt-funds-target.LLh9Z1 \
  CARGO_BUILD_JOBS=4 RUST_MIN_STACK=16777216 \
  AETHER_TEST_DATABASE_URL='postgresql://fengying@localhost/postgres?host=/tmp/aeris-gateway-funds-db.Mh5Mzm&port=57419' \
  /Users/fengying/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin/cargo test \
  -p aether-gateway --lib execution_runtime::funded_image::tests -- \
  --include-ignored --test-threads=1 --nocapture
```

原有回归使用同一最终 test binary 和 `RUST_MIN_STACK=16777216`，分别筛选 `execution_runtime::sync::execution::tests`、`executor::candidate_loop::tests`，均以 `--test-threads=1` 执行。数据库实例验收后正常停止，目录和编译产物保留，方便主线程复核。
