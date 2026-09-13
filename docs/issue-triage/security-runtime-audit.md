# #205-#218 runtime/security audit

审计基线：当前 fork `main`，HEAD `12a1d265c`（2026-09-12）。本表按源码和现存测试复核，`当前成立` 表示仍可由当前代码触发，`已修复` 表示 issue 的旧描述已被实现和测试覆盖，`设计取舍` 表示行为是显式策略而非缺陷。旧 issue 的 P0 标签不因历史描述自动保留。

## 优先级总览

| 优先级 | 当前仍需处理 |
| --- | --- |
| P0/P1 | #205 发布制品缺独立签名信任锚；#206 图片授权成本估算/信用额度、`insufficient_quota` 补偿、非有限公式和敏感 header 配置断链；#211 OpenAI 状态/字段、registry retention 与租约失效截流；#214/#217 `/readyz` 不反映依赖；#216 真实 PostgreSQL 测试变量未接入；#218 compose 未显式 production。 |
| P2 | #205 regex/hash/未接线 admission；#209、#210、#212、#213、#215 的性能、维护性和深防御项；#217 监控资产；#220 CI 漏洞扫描。 |
| 已关闭/降级 | #205 重定向泄密、回放无界、私网目标默认值、降级；#206 enrichment 失败写零；#207 错误泄露、优雅停机、Windsurf 无界 channel、RPM fail-open；#208 NUMERIC CAST/LIKE；#209 凭据日志和 Debug；#211 明文落盘；#212 容量曲线非 2xx；#216 VSCodex CI；#218 JWT fallback、compose secret guard、镜像 digest。 |

## #205 隧道与调度

| 子项 | 当前判定/优先级 | 当前证据 | 测试证据 | 建议 |
| --- | --- | --- | --- | --- |
| 升级完整性仅同源 SHA256 | 当前成立，P0/P1 | `apps/aether-tunnel/src/setup/upgrade.rs:409-451` 下载 archive 与同一 release 的 `SHA256SUMS.txt` 并校验；无签名密钥或独立信任根。 | 现有测试只覆盖 checksum 成功/失败路径。 | 拆为独立 signed-release/provenance issue。 |
| 降级保护 | 已修 | `apps/aether-tunnel/src/tunnel/heartbeat.rs:352-366` 拒绝目标版本 `<= CURRENT_VERSION`。 | heartbeat 升级测试覆盖版本比较。 | 关闭旧子项。 |
| 远程升级 root/systemd | 设计取舍，P2 | `setup/service.rs:333-368` unit 未设 `User=`，自升级需要 root。 | service 生成测试。 | 若要降权，另做升级/权限架构设计。 |
| 重定向携带凭据 | 已修 | `stream_handler.rs:1209-1216,1267-1269` 比较 scheme/host/port，跨源停止。 | `stream_handler.rs:3005-3023` HTTPS→HTTP 回归测试。 | 关闭。 |
| replay body/内存预算 | 已修 | `stream_handler.rs:577-660` 单请求 chunk/budget 与全局 256 MiB 上限。 | `:2748-2763` 超预算测试。 | 关闭。 |
| 私网目标默认值、DNS key、IPv6 | 已修 | `config.rs:291-298` `allow_private_targets=false`；`target_filter.rs:42-47,87-165` key 含 host/port/policy；统一 `aether_http::is_private_or_reserved_ip`。 | target_filter 私网、IPv4-mapped/transition 和缓存隔离测试。 | 关闭。 |
| model regex 热路径 | 当前成立，P2 | `crates/aether-scheduler-core/src/model.rs:417-429` 每次调用 `RegexBuilder::build`。 | 单元测试验证匹配语义，无基准。 | 缓存编译 regex 或增加 benchmark。 |
| pool hash 确定性 | 当前成立，P2 | `crates/aether-pool-core/src/scheduler.rs:599-604` 使用 `DefaultHasher`，跨进程稳定性未契约化。 | scheduler 排序测试。 | 若需跨进程一致，改显式 hash。 |
| unknown rank fallback | 当前成立，P2 | `scheduler.rs:391-397` 未知 rank 回退 0。 | 基础排序测试。 | 明确未知值策略并加生产输入测试。 |
| dispatch `mark_current` 非 Pending 仍可覆盖 | 当前成立，P2 | `crates/aether-dispatch-core/src/sequence.rs:81-89` 直接给 cursor 项写 mark；`next()` 才会跳过非 Pending。 | `sequence.rs:97+` 仅覆盖正常推进。 | 增加重复 mark 回归或以类型状态机约束。 |
| allowed provider 按 id/name/type 匹配 | 设计取舍，P2 | `aether-scheduler-core/src/auth.rs:8-35` 任一字段匹配即放行；`allowed_providers=["openai"]` 会覆盖同类型未来 provider。 | `auth.rs:108+` 测试锁定该语义。 | 文档明确 type 是通配授权，或分离 ID/type selector。 |
| adaptive RPM 无 429 时间时置信度归零 | 设计取舍，P2 | `scheduler-core/src/health.rs:396-417` `last_429=None` 时 `time_decay=1.0`；正常 projection 同时写 learned limit/429，异常组合主要来自迁移/人工数据。 | `health.rs:1152+` 置信度测试。 | 补异常历史数据测试与指标，不按生产绕过定级。 |
| stream id 重复 | 已修 | `apps/aether-tunnel/src/tunnel/dispatcher.rs:69-80,257-265` 在插入前拒绝 0 和已占用 stream id。 | `dispatcher.rs:778-808` duplicate-id 测试。 | 关闭旧子项。 |
| permit 在 response relay 前释放 | 已修，P1（#205/#214 capacity isolation） | `apps/aether-tunnel/src/tunnel/stream_handler.rs:1899-1906` 将 `AdmissionPermit` 保留至 `handle_stream_inner` 返回；direct、followed redirect、不可回放 redirect、body error/timeout、writer failure 及 task cancellation 均由同一 RAII guard 释放。 | `stream_handler.rs` 中 `admission_permit_covers_response_body_and_redirect_terminal_paths`（direct/follow/unreplayable）和 `cancelling_response_body_releases_local_and_distributed_admission`；45 个 stream-handler tests 全部通过。 | stream admission 现在表示完整 tunneled stream；按真实响应体并发容量配置，继续保留 #205/#214 其余签名、身份与 readiness 任务。 |
| Windows 运行中 exe 替换 | 当前成立，P2 | upgrade 路径仍依赖 rename/replace，Windows 对运行中 exe 的覆盖语义可能失败。 | 现有测试未覆盖 Windows 真实升级。 | Windows runner 做端到端升级或明确不支持。 |
| send/emergency/admission ledger | 设计取舍，P2 | `aether-scheduler-core/src/send_admission.rs:533-542` 明确在 authority 接线前 fail-closed；`emergency_chain.rs:530-541` ledger proof 同样预留；`aether-admission-core/src/budget.rs:27-75` body bytes 当前为 0。 | 各 gate 纯函数测试。 | 记录为未接线能力，不把空实现误报为绕过。 |

## #206 计费与结算

| 子项 | 当前判定/优先级 | 当前证据 | 测试证据 | 建议 |
| --- | --- | --- | --- | --- |
| enrichment 失败仍写 0 元 | 已修 | `crates/aether-usage/runtime/src/worker.rs:148-165,809-831` enrich 使用 `?`，失败直接返回；retryable/permanent 分别重试/DLQ（`:699-742`）。 | `worker.rs:1568,1709` 及 `runtime.rs:8707-8826` 断言失败不持久化。 | 关闭旧子项。 |
| cancelled 请求是否收费 | 设计取舍 | `contracts/.../usage/metadata_policy.rs:23-30` 默认 `cancelled_request_fee=false`；`usage/runtime/src/record.rs:223-232` flag=false 为 void，true 才 billable。 | `record.rs:513-568` 同时锁定默认 void 与显式收费。 | 产品决定首字节后何时设置 flag；拆策略 issue。 |
| 图片授权成本估算 | 当前成立，P0/P1 | `crates/aether-billing/src/service.rs:48-61` `estimate_authorization_cost_upper_bound` 对 `task_type=image` 直接 `Ok(None)`；实际结算支持 image price matrix/range（`:464-565,631-855`），网关 gate 在 `gate.rs:419` 调用估算。 | image 结算测试（service `:1835+`）未覆盖授权 gate 上界。 | 实现图片上界/保守拒绝并补 gateway 集成测试。 |
| daily quota/wallet overdraft | 当前成立，P1 | `settlement.rs:628-633` 依据 entitlement、wallet availability、`wallet_can_overdraft` 判定；`gate.rs:219-235` 合并 quota/wallet。 | settlement live tests `:1573-1679` 覆盖并发扣额度；缺少不足后充值追扣契约。 | 明确不足额度后的补偿/追扣流程。 |
| `insufficient_quota` 终态不可重算 | 当前成立，P1 | `settlement.rs:1131-1137` 将 `insufficient_quota` 与 settled/void 一样早退；`:1294-1324,1388-1402` 直接 finalize。 | 现有 settlement tests 覆盖终态写入，未覆盖充值后重试。 | 产品决定是否允许补偿结算，拆 issue。 |
| 支付回调 f64::EPSILON | 已修 | 公共 helper `aether-data/contracts/.../wallet/types.rs:1173-1238` 使用 `1e-6`、两位重建和有限正数校验，三后端复用。 | 同文件 `:3553-3637` 覆盖 legacy/官方 callback 组合。 | 关闭旧子项。 |
| API format `rate_multiplier` 负/零 | 当前成立，P1/P2 | `billing/pricing.rs:442-458` 直接接受 JSON f64，缺失回退 1.0；`service.rs:263-269` 直接乘入结算。授权 gate 会把非法 API-key multiplier 回退 1.0（`gate.rs:137-147`），但 billing snapshot 路径仍可能负/零。 | processing-tier multiplier 已有 finite/non-negative 测试，但该 map 的负值缺回归。 | 写配置时拒绝 `<=0` 或定义 0 的免费语义。 |
| 公式除零/非有限结果 | 当前成立，P1/P2 | `formula_engine.rs:165-199,595-613` `/`, `//`, `%` 可产生 inf/NaN，`cost < 0` 对 NaN 不生效；`precision.rs:4-9` 透传非有限值，最终结算 finite 校验会将事件送 DLQ。 | 公式测试未锁定所有非有限操作。 | evaluator 立即拒绝非有限中间值/结果并在配置发布时验证。 |
| Redis stream `MAXLEN ~ 200000` | 当前成立，P1/P2 | `usage/runtime/src/config.rs:17,50,129-132` 默认 200k；`queue.rs:108-112` 以近似 maxlen append。积压超过阈值会驱逐最旧计费/审计事件。 | queue 测试验证传入 maxlen；现有 dropped counter 主要覆盖进程内 deferred buffer，不等同 Redis 裁剪。 | 增加 stream 长度/裁剪告警，按账单耐久目标配置。 |
| `sensitive_headers` 配置断链 | 当前成立，P1 | admin 默认值在 `aether-admin/src/system.rs:2235-2241`，网关/usage 无该配置消费者；`usage/policy.rs:370-372` 只验证 headers 是 object，测试 `:835-853` 允许 authorization 原样存在。当前 body masking 已改成完整 `[redacted]`，但自定义 header 策略仍未接线。 | persistence policy 测试目前锁定“原样保留 object”。 | 统一采集前脱敏入口并消费管理员配置；默认覆盖 Google/自定义 auth header。 |
| `u64 as i64` 图片 token 转换 | 当前成立但不可达，P2 | event enrichment 仍有直接窄化转换；正常 provider token 数远小于 `i64::MAX`。 | 用边界值单测替代直接 cast。 | 使用 `try_from`，不列资损主线。 |

## #207 网关核心

| 子项 | 判定 | 证据/测试 | 建议 |
| --- | --- | --- | --- |
| Internal 错误泄露 | 已修 | `apps/aether-gateway/src/error.rs:324-357` 对外固定 internal message，内部记录指纹/脱敏；`:436-450` 测试。 |
| 优雅关闭/连接 drain | 已修 | `main.rs:1890-1936,1954-1959,2597-2634` cancellation-aware accept、drain、force-close；`shutdown_tests.rs`。 |
| Windsurf 无界 channel | 已修 | `execution_runtime/windsurf.rs:589` bounded channel，`:750-757` full 明确报错；Windsurf 测试覆盖。 |
| RPM backend fail-open | 已修 | `main.rs:1283-1286` 默认 false；`rate_limit.rs:227-245` 仅显式开启才 fail-open。 |
| loop guard/URL secret 回显 | 已修 | `frontdoor_loop_guard.rs:71-151` 规范化 loopback/path；`transport.rs:6215-6275` 测试 secret 不回显。 |
| frontdoor body 软预算/字符串错误分类 | 已修/降级，P2 | `crates/aether-gateway/frontdoor/src/body.rs:120-284` 用显式 reservation、capacity 和 hard limit；当前源码不再匹配旧的 `"length limit exceeded"` 文案。 | `body.rs:529+` 覆盖声明/未知长度、压缩和预算边界。 |
| crate 级 lint allow | 当前成立，P2 | `apps/aether-gateway/src/lib.rs:1-26` 仍全 crate allow dead_code/unused 与多项 clippy。 | CI 编译无法发现这些类别。 | 逐类收窄，不与安全修复混合。 |
| OAuth success effect 裸 spawn/凭据驻留 | 当前成立，P2 | `orchestration/effects.rs:278-325` 克隆 bearer authorization 后 `tokio::spawn`，不受 TaskSupervisor 管理。 | effect 功能测试；缺 shutdown/drop 测试。 | 使用 supervisor，缩短凭据所有权周期。 |
| per-target gate map 增长 | 当前成立但有界，P2 | `upstream_admission.rs:24-29,104-109` DashMap 无显式淘汰，key 受 provider/target 配置数量约束。 | admission/metrics 测试。 | 配置变更时清理或加 TTL。 |
| SingletonLeaseGuard runtime-close 释放 | 当前成立，P2 | worker lease drop 依赖可用 Tokio handle；runtime teardown 时最终依靠 TTL。 | worker lease 单测。 | 提供显式 release 并监控 TTL fallback。 |

## #208 数据与返利

| 子项 | 判定 | 证据/测试 | 建议 |
| --- | --- | --- | --- |
| PostgreSQL NUMERIC → f64 | 已修 | `runtime/src/backend/referrals.rs:176-179` 宏仍取 f64，但所有读取 SQL 已 `CAST(... AS DOUBLE PRECISION)`，见 `:1453-1456,1560-1571,1650-1721,1794,1820,2105-2109,2167-2177,2308-2318,2369`。 |
| LIKE 通配符注入/误匹配 | 已修 | referrals 查询使用 `ESCAPE '!'`（如 `:1294-1296` 及同类查询）。 |
| legacy 明文 key 优先 | 当前成立/迁移策略，P2 | `provider_catalog.rs:161,220` 仍为 `COALESCE(api_key, encrypted_key)`；写入/CAS 在 `:2301-2304` 把值迁到 `api_key` 并清 `encrypted_key`。字段名与历史 issue 的“明文/密文”假设已不再可靠。 | `:4056-4059` 测试锁定迁移 CAS。 | 先确认 schema 加密语义再做列清理，不能按旧描述直接删列。 |
| provider row `.ok()` 吞 decode 错 | 当前成立，P2 | `provider_catalog.rs:3716-3744` 多个可选列 `try_get(...).ok()`，类型漂移会静默变 None。 | row mapping 测试覆盖正常值，缺类型错误。 | 必需列传播错误；真正兼容列显式注释/指标。 |
| LIKE/非 ASCII 后端差异 | LIKE 已修，Unicode 设计取舍 | referrals 查询已有 `ESCAPE '!'`；旧三方言 query/SQLite 路径在当前树中已不存在，不能沿用旧行号。 | referrals 搜索测试。 | 如仍支持多后端，从当前 adapter 重新建立 parity 用例。 |
| PostgreSQL rollback 错误被吞 | 当前成立，P2 | `adapters/postgres/src/tx.rs:102-111` 主操作失败后 `let _ = tx.rollback().await`。 | transaction 测试关注原始错误，未验证 rollback 日志。 | 保留原错误但记录 rollback failure。 |
| TTL map zero 语义 | 当前成立，P2 | `aether-cache/src/ttl_map.rs:94-103` ttl=0 返回 true 但不插入；`:266-279` max_entries=0 禁用容量淘汰。 | ttl_map 单测覆盖常规 TTL。 | 明确 0 是 disable/unbounded 还是非法并统一 API。 |
| 返利 insert→credit 跨事务 | 已有自动补偿，设计取舍 | `referrals.rs:749-758,993-1021,2075+` 首次 credit 后，后台扫描 pending/failed 并幂等重试；不再仅靠人工。 | retry 状态、pending reversal/cap 单测（`:2646+,:2828+`）。 | 保留监控/告警，关闭“无自动恢复”旧描述。 |
| memory 锁中毒、管理员负向余额、f64 账本 | 设计取舍，P2 | memory repository 主要为测试；管理员调整与二进制浮点是长期账本策略。 | memory/钱包测试锁定现语义。 | 另立产品/长期迁移 issue，不与 CAST P0 混合。 |

## #209 Provider 与凭据

| 子项 | 判定 | 证据/测试 | 建议 |
| --- | --- | --- | --- |
| 视频 follow-up URL、loop guard 日志泄密 | 已修 | `ai_serving/planner/decision/sync.rs:235-251` 只记 origin；`frontdoor_loop_guard.rs:71-82` 脱敏；transport 测试 `:6274`。 |
| Transport key/Vertex/OAuth Debug | 已修 | `provider/transport/snapshot.rs:122-170` sensitive skip + `[REDACTED]`；`vertex/auth.rs:25-61`；`oauth_refresh/mod.rs:71-85`。 |
| quota 时间戳启发式、模糊封禁词、registry 重建、Kiro prompt/tool index、SA token cache | 当前成立，P2/设计 | 属稳健性或产品策略，当前未见直接凭据泄露。 | provider 现有单测。 | 拆 P2，明确缓存失效与词表契约。 |

## #210 加密/OAuth

| 子项 | 判定 | 证据/测试 | 建议 |
| --- | --- | --- | --- |
| Fernet 固定 salt | 设计取舍，P2 | `crates/aether-crypto/src/python_fernet.rs:34-43` 为兼容 Python Fernet；已有直接 key/fallback rotation。 | crypto round-trip/rotation 测试。 |
| OAuth HttpStatus 泄露 body | 已修 | `aether-oauth/core/error.rs:10-17,24-93` Display 不含 body、Debug 为 `[REDACTED]`；custom OIDC `:108-112,151-155` 使用 excerpt helper。 |
| tunnel sequence load/store 非原子 | 当前成立，P2 | `aether-contracts/src/tunnel_security.rs:147-170` 分离 load/store；调用方串行。 | sequence 单测。 | 如需并发安全，改 CAS/事务并补多线程测试。 |
| OAuth HTTPS scheme 深防御 | 当前成立，P2 | endpoint policy 仍需在 `apps/aether-gateway/src/oauth/http_executor.rs` 统一 enforce。 | 现有 URL 校验测试不覆盖所有自定义 endpoint。 |
| OAuth state/id_token crate 契约 | 设计取舍，P2 | identity crate 本身透传 state、custom OIDC 以 userinfo 为身份源；生产 gateway 在外层以单次 nonce + provider type + PKCE 校验。 | gateway OAuth callback 测试覆盖生产闭环。 | 可将 state/HTTPS 作为 crate 强制参数；无需按现役 CSRF 漏洞定级。 |
| 通用 OAuth executor body/network policy | 不可达/深防御，P2 | `aether-oauth/src/network/executor.rs:72-92` 通用 executor 仍无统一 body 上限，但生产使用 gateway executor；旧风险无生产调用点。 | gateway executor 有 proxy/timeout/response-limit 测试。 | 限制公开 API 或复用 policy executor。 |
| callback fragment 覆盖 query | 不可达，P2 | `aether-oauth/src/core/pkce.rs:25-55` helper 行为仍在，但生产 gateway 用自己的 query parser。 | pkce helper 单测。 | 修 API 语义或移除未用 helper。 |

## #211 视频任务/runtime/admin

| 子项 | 判定/优先级 | 证据/测试 | 建议 |
| --- | --- | --- | --- |
| upstream key/prompt 明文落盘 | 已修 | `video-tasks-core/src/store_backend.rs` 强制 Fernet v2、拒绝明文并执行 legacy migration；sidecar lock、CAS、原子替换和 owner-only 文件权限；读取时收紧历史宽松权限并拒绝 symlink。`types.rs` 的 persistence/seed Debug 对原始请求体、prompt、provider diagnostics、metadata 和 video URL 脱敏。 | 关闭旧 P0；补强验收记录见 [`video-task-secrets-decision.md`](video-task-secrets-decision.md)。 |
| 内存→磁盘失败分叉 | 已修 | `store_backend.rs:221-233` copy-on-write，持久化成功后替换内存。 | 关闭。 |
| OpenAI `in_progress` 状态 | 当前成立，P1 | `video-tasks-core/src/openai.rs:94-108` 只识别 `processing`，未知（含 `in_progress`）回退 Submitted。 | 增加映射并补 provider 状态回归测试。 |
| OpenAI 增量响应清空旧字段 | 当前成立，P1/P2 | `openai.rs:118-133` 在响应缺字段时无条件把 completed/expires/error/video URL 更新为 None。 | 缺少稀疏轮询响应保留字段测试。 | 仅在字段存在时更新，终态显式清理。 |
| registry 终态 retention/写放大 | 当前成立，P1 | `store_registry.rs:11-27` BTreeMap 无界；`:66-83` Cancel/Delete 只改状态不删除；每次写序列化全 registry。 | 增加 TTL/compaction/分页持久化策略。 |
| admin 非 ASCII password mask | 已修/设计收敛 | `aether-admin/src/system.rs:3286-3354` 管理员 payload 不再返回 `proxy_password`，仅给 `has_proxy_password`；`:3883-3920` 验证敏感字段不出现在响应。旧的按字节切片 panic 行号已不存在。 | 可补 Unicode 输入→列表响应回归；无需恢复旧 mask 实现。 |
| cookie fallback、retry jitter | 当前成立，P2 | `aether-admin/src/provider/ops/verify.rs:44-53` 找不到精确 cookie 段时返回整串；`aether-http/src/retry.rs:11-16` 使用 SystemTime 纳秒并可能超过 max delay。 | 精确解析失败应报错；jitter clamp/可注入 RNG。 |
| distributed lease 失效导致流式响应干净截断 | 当前成立，P1 | `aether-runtime/base/src/admission.rs:142-183` 健康租约失效时直接结束 body stream；`handlers/proxy/finalize.rs:178`、`execution_runtime/server.rs:725-751` 为生产接线。Redis 清理使用本机 `unix_time_ms()`（`runtime/state/src/redis/runtime.rs:916-967`），跨节点时钟偏差会放大误剪/漏剪。 | admission permit 健康轮询测试；缺少客户端可识别的终止帧契约。 | 发送明确错误/终止事件并统一 Redis 时间源。 |
| TaskSupervisor 被 drop 时内部任务 detached | 当前成立但生产已规避，P2 | HEAD 的 `aether-task/runtime/src/lib.rs:285-307,310-336` JoinSet 包装任务内部另行 spawn，类型未实现 `Drop`；但 gateway `main.rs:2640-2648` 在传播 `serve_result` 前显式 shutdown，旧的直接错误返回路径已修。 | HEAD `lib.rs:368-416` 只测显式 shutdown、panic/abort，未测 drop。 | 为库 API 实现 Drop 取消并补回归，按深防御而非现役 P1 定级。 |
| lease 基础类型/supervisor 观测 | 部分已具备，P2 | `aether-task/core/src/lease.rs:1-33` 提供过期和 fencing 校验；`aether-task/runtime/src/lib.rs:141-178` 提供 supervisor metrics。 | 单元测试覆盖过期、fencing、metrics。 | 与上条拆分，避免把已有能力误判为完整生命周期管理。 |
| metrics 锁中毒归零、log cleanup 裸任务 | 当前成立，P2 | TaskSupervisor metrics snapshot 锁失败回 default；`aether-runtime/base/src/tracing.rs` cleanup task 仍由 tracing 初始化独立 spawn。 | metrics 正常路径测试，无 poisoned/drop lifecycle 测试。 | 保留上次快照/暴露错误，统一 task ownership。 |
| memory/Redis stream trim 语义 | 当前成立，P2 | memory backend 裁剪会同步移除 group PEL，Redis `XADD MAXLEN` 不保证同语义。 | 两后端各自测试，缺共享契约套件。 | 建立 backend parity 测试并文档化差异。 |
| `DistributedConcurrencyGate` 命名 | 设计债，P2 | runtime/base 导出的同名类型实际是进程内 semaphore，当前无生产调用方。 | 单元测试仅验证本地语义。 | 删除或改名，避免未来误用。 |
| `created_at_unix_ms` 实存秒、kv ttl=None | 设计/命名，P2 | 当前链路单位自洽；kv 的 None 调用方为固定键覆盖，无无界增长证据。 | 现有任务/kv 测试。 | 渐进重命名并明确 None 契约。 |

## #212 测试与依赖

| 子项 | 判定 | 证据/测试 | 建议 |
| --- | --- | --- | --- |
| 容量曲线忽略非 2xx | 已修 | `capacity_curve_baseline.rs:444-515` 仅 2xx 计成功，失败/拒绝/延迟触发 saturation。 |
| tokio-tungstenite/socket2 版本漂移 | 当前成立，P2 | tunnel `Cargo.toml:20,43` 为 0.24/0.5；gateway/workspace `:90` 与 `Cargo.toml:129` 为 0.28/0.6；Cargo.lock 同时存在双版本。 |
| 其他 crate 绕过 workspace version | 当前成立，P2 | axum/rsa/clap/sysinfo 等仍有 crate 级直接版本；当前不一定分叉，但缺一致性门禁。 | Cargo.lock/CI 只能暴露已发生的分叉。 |
| 测试端口 TOCTOU | 当前成立，P2 | `testing/support/src/lib.rs:96-98`、`testing/testkit/src/server.rs:59-61`、`redis.rs:111-113`、`postgres.rs:209-211` 先探测再绑定。 |
| ManagedRedisServer 双实现 | 当前成立，P2 | testing support/testkit 各有被调用的近重复实现。 | Redis 集成测试。 |
| throughput 与 SSE load parser | 当前成立，P2 | loadtools 吞吐含快速失败 completed；SSE 单行/多行解析有 4 KiB 和拼接边界。 | loadtools 单测覆盖常规格式，缺规范边界。 |
| Prometheus parser 边界 | 当前成立，P2 | test metrics helper 支持的名称/行格式比 Prometheus 暴露格式窄。 | parser 单测。 |
| 手写 argv、test server panic | 当前成立但仅测试，P2 | 多个 baseline bin 重复解析 argv；`SpawnedServer::start_on_port` 仍以 expect 报 serve 失败。 | 测试基础设施。 |
| frontend XSS/token、构建耦合 | 设计取舍，P2 | CodeHighlight 依赖 hljs escaping，access token 在 localStorage；frontend prebuild 同步 vscodex 产物。 | frontend/vitest；VSCodex CI 已接入（见 #216）。 |

## #213 代码质量

| 子项 | 判定 | 证据 | 建议 |
| --- | --- | --- | --- |
| 巨型模块/函数 | 当前成立，P2 | 当前 `main.rs` 5084 行、`execution_runtime/transport.rs` 9837 行、`settlement.rs` 1741 行（以 HEAD 实测）；结构性维护风险。 | 以边界和测试为单位渐进拆分。 |
| missing docs/lint | 当前成立，P2 | workspace 未将缺失文档作为发布门禁；公共 API 仍可增量补注释。 | 单独 DX issue。 |
| DataLayerError 携带数据库字符串 | 当前成立，P2 | `crates/aether-data/contracts/src/error.rs:1-35` Postgres/Redis/Sql 变体保存 `String`；调用方需统一脱敏日志。 | 对外错误保持通用，内部日志走 redact helper。 |

## #214 可用性

| 子项 | 判定/优先级 | 证据/测试 | 建议 |
| --- | --- | --- | --- |
| `/readyz` 静态常绿 | 已部分修复，#308 收口中 | 主干已有 DB/Redis 探测；PR #356 增加共享总 deadline、缓存、启动/关闭和关键 worker gate，并已有真实依赖演练证据。 | 待 #356 合并后完成 #308 验收。 |
| `/health` 与依赖检查职责 | 明确区分 liveness/readiness | `/health` 的进程存活契约不应因依赖故障失败；`/_gateway/health` 是遥测快照。PR #356 修复 `/health` 经代理 IP 检查间接访问 Redis 的路径。 | 负载均衡摘流使用 `/readyz`。 |
| SIGTERM/drain | 已修 | `main.rs:1890-1936,1954-1959,2597-2634` 支持 cancellation、drain deadline 与 force close；`shutdown_tests.rs` 覆盖。 |
| accept error retry/backoff | 已修 | `crates/aether-gateway/frontdoor/src/connection.rs:113-146` 对资源错误 1 秒 backoff，连接错误立即重试；有 connection_tests。 |
| 分布式限流与 RPM 故障策略矛盾 | 原默认策略结论已过时 | RPM 默认 `fail_open=false`；多节点禁止 local fallback，单节点可使用本地限流回退；显式 `fail_open=true` 才完全放行。 | 剩余是 Redis 故障错误契约、恢复与注入证据。 |
| per-upstream bulkhead 默认值 | 当前成立，P1，容量切片实施中 | 全局请求容量已受 CPU/FD 限制；四 CPU/65536 FD 时 global=4096、target auto=10000，且降低显式 global 不联动 target。 | [自动容量决策](issue-214-target-capacity.md)；流式/同步 permit 生命周期仍单独验收。 |
| stream idle timeout | 已修 | `execution_runtime/stream_read_timeout.rs:8-30` 默认 300s，可显式 0 禁用；`:78-100` 测试。 |
| DB 故障客户端契约 | #340 已完成 | PR #344 已合并，将列举的控制面依赖错误映射为 502、trace_id、Retry-After 与稳定 code。 | 保留独立 Redis admission 错误契约验收。 |
| statement_timeout | 普通连接默认已实现 | `adapters/postgres/src/pool.rs` 普通连接为 statement 30 秒/lock 3 秒；timeout=0 仅属于用后销毁的迁移连接。 | 将已有真实 SQL timeout/迁移隔离测试纳入常规 CI；服务器超时不等于客户端 TCP 黑洞有界。 |
| pool key 分布式 lease/owner forward timeout | 待确认，P2 | 属 issue 的架构疑问，当前没有足够生产故障证据；跨节点路径已有独立 Redis in-flight/客户端 timeout。 | 建议另做多节点故障注入，而非直接修改。 |

## #215 性能

| 子项 | 判定 | 证据/测试 |
| --- | --- | --- |
| 同步日志阻塞 | 已修 | `aether-runtime/base/src/tracing.rs:620-665` stdout/file 均 NonBlocking writer + worker；root logging 测试。 |
| provider RPM reset 全局 Mutex | 当前成立，P2 | `apps/aether-gateway/src/state/core.rs:768-789` 每次查询都 lock + retain；state 字段 `state/app.rs:466`。 |
| `direct_passthrough_mode` 读 env 热路径 | 当前成立，P2 | `execution_runtime/stream/execution.rs:397-405`，调用点 `:2907,3020`。 |
| serde_json preserve_order | 设计取舍，P2 | workspace `Cargo.toml:126` 全局开启，兼容输出顺序但增加开销。 |
| SSE observer 全行解析/拷贝 | 当前成立，P2 | `stream_pump.rs:767-786,820-860` 非私有流仍 `chunk.to_vec()`，每个换行事件送 parser；未见针对纯内容 delta 的字节级预过滤。 | observer 有超长行/terminal correctness 测试。 |
| failover 每次深 clone JSON body | 已修/重构后不成立 | 当前 transport 在 `:3188-3203` 借用 `json_body.as_ref()` 并经 bounded serializer；旧 `json_body.clone()` 热点已不存在。 | `transport.rs:7560+` 覆盖有限序列化。 |
| SSE/响应体预算与 idle | 已修/部分成立 | privacy SSE 有事件/恢复预算（`privacy/mod.rs:3079-3412`，多项测试）；upstream response 有 wire/decompression limit（`transport.rs:955-1020,6156-6199`）。 |
| gzip/br 同步解码 | 当前成立，P2 | `transport.rs:5280-5305` 用同步 flate2/brotli decoder；大响应虽有字节上限，仍占 Tokio worker CPU。 | decompression bomb/边界测试。 |
| 单体 body 上限 | 已修 | headers/body 路径有默认和硬上限、全局内存预算；README 已说明 256MB 默认。 | headers/frontdoor body 边界测试。 |

## #216 CI/DX

| 子项 | 判定/优先级 | 证据/建议 |
| --- | --- | --- |
| VSCodex CI | 已修 | `.github/change-filters.yml:26`、`.github/workflows/frontend-ci.yml:158-197` 已 install/build/check/test。 |
| live PostgreSQL 变量不一致 | 当前成立，P1 | workflow `rust-ci.yml:555-586` 只设置 `AETHER_TEST_POSTGRES_URL`；大量 ignored adapter tests（如 `usage/tests.rs:192-195,367-370`）要求 `AETHER_TEST_DATABASE_URL`；workflow 未执行 `--ignored`。 | 统一变量并建立真实 settlement/wallet gate。 |
| build.rs 依赖 `.git/HEAD` | 当前成立，P2 | `apps/aether-gateway/build.rs:4-29` 读取 git 元数据；源码包/浅克隆需 fallback。 |
| CI 漏洞扫描 | 当前成立，P1/P2 | workflow 未见 cargo-audit/cargo-deny/npm audit/osv-scanner。 | 添加固定版本的依赖/镜像扫描门禁。 |

## #217 观测性

| 子项 | 判定/优先级 | 证据/建议 |
| --- | --- | --- |
| readiness 与依赖状态 | 当前成立，P1 | 与 #214 相同：`api/core.rs:90-99` 静态 ready。 |
| billing/wallet 错误 metrics | 当前成立，P1/P2 | usage runtime 有 pipeline counters（`runtime.rs:3656+`），但 enrichment/settlement 失败点主要是日志（`worker.rs:820-829`、`runtime.rs:5224-5234`），未见独立计数器。 | 增加可告警 counter、失败原因标签白名单。 |
| 请求 RED metrics | 部分成立，P2 | 阶段/tunnel/background metrics 存在，未发现完整 request rate/error/duration 三元组。 | 统一 middleware 指标。 |
| Prometheus/Grafana/告警资产 | 当前成立，P2 | `rg --files` 仅见代码 metrics 与 frontend prometheus helper，未见完整部署 dashboards/alerts。 | 提供最小 production alert bundle。 |
| 默认 pretty log | 设计取舍，P2 | `docker-compose.yml:82-83` 默认 pretty；生产可通过 env 改 JSON。 | 文档/部署 profile 明确生产日志格式。 |

## #218 配置与部署

| 子项 | 判定/优先级 | 证据/测试 | 建议 |
| --- | --- | --- | --- |
| JWT fallback/placeholder | 已修 | `local_auth_token.rs:5-10,30-56` 拒绝缺失、过短、不安全默认值；`:292+` 测试。 | 关闭旧子项。 |
| compose secret guards | 已修 | `docker-compose.yml:13,48,76-77` 使用 `${...:?}`；`.env.example:42,47` 不再填弱默认密码。 | 关闭。 |
| ENVIRONMENT 默认 development | 当前成立，P1/P2 | `main.rs:1255` 默认 development；compose `environment`（`:74-85`）未显式设置。JWT 已独立校验，但 CORS/cookie posture 仍受环境影响。 | compose 显式 `ENVIRONMENT=production`，并为生产配置做启动检查。 |
| ENCRYPTION_KEY 启动校验 | 已修 | `main.rs:756-760` 校验函数；正常启动 `:2204`，export/import `:2170-2175`，migrate/backfill 亦调用。 | 关闭“未校验”旧描述；补配置文档。 |
| body limit | 已修 | README `:137` 说明默认 256MB 与硬上限；headers 预检 declared Content-Length。 | 关闭。 |
| base image digest | 已修 | `Dockerfile.app:13,30` busybox/distroless 均 pin digest。 | 关闭，保留更新流程。 |
| container root/no-new-privileges | 设计取舍，P2 | `Dockerfile.app:47` `USER 0:0`；compose 有 no-new-privileges、tmpfs、最小 capabilities。 | 若改非 root，另做文件权限/升级设计。 |
| Redis Streams 默认不持久化 | 设计取舍，P1/P2 | `docker-compose.yml:44-48` Redis 使用 `/tmp`、`appendonly no`、`save ""` 且无 volume；usage stream/DLQ 在 Redis 故障或重建时丢失。 | 提供 durability profile，并在计费生产部署强制选择。 |
| compose 资源限制 | 当前成立，P2 | 主 compose 未配置 CPU/memory/pids 限制；DB/app 可能互相争抢宿主资源。 | 给生产 override 示例，不强加本地默认。 |
| 配置双轨/非法 env 静默 fallback | 当前成立，P2 | clap 强类型参数之外仍有请求路径 `std::env::var(...).ok().and_then(parse)`；例如 gate auto/metrics 配置。 | 收口到启动快照并打印 redacted effective config。 |
| 环境变量文档覆盖率旧统计 | 当前成立，P2 | 旧 issue 的百分比未按当前 HEAD 重算。 | 重新生成清单，避免引用过期数字。 |

## 附录：#220 供应链交叉项

| 子项 | 判定 | 证据/建议 |
| --- | --- | --- |
| 根 installer checksum | 已修 | 根 `install.sh:738-761,1462-1476` 下载 `SHA256SUMS` 并校验；tunnel installer 亦有对应逻辑。 |
| image digest | 已修 | `Dockerfile.app:13,30`。 |
| CI vulnerability scan | 当前成立，P1/P2 | `.github/workflows` 未发现 cargo-audit/cargo-deny/npm audit/osv-scanner；与 #216 合并跟踪。 |
| 独立签名/发布 provenance | 当前成立，P0/P1 | installer 仅信任同源 SHA256，见 #205；需要签名 manifest、离线/固定 trust root 与轮换流程。 |
| runtime root | 设计取舍，P2 | `Dockerfile.app:47`；已有 no-new-privileges/tmpfs/cap 约束。 |

本审计只新增此文档；未修改代码、测试、issue 状态或远端分支。
