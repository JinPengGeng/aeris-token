# Issue #205 残余风险接受记录（隧道 agent 与调度准入）

状态：Accepted（风险接受）。日期：2026-09-22。维护规则见 [ADR 索引](README.md)。
关联：Issue #205（深度评审）、#477（部分交付：正则缓存、自适应 RPM 置信度分支测试、隧道信任模型文档）。

## 决定

对 Issue #205 深度评审中尚未交付代码加固的 7 个残余项，维护者于 2026-09-22 拍板
**接受风险、仅文档化（原选项 3）**，不实施对应的安全/健壮性代码改动。本文逐项记录
风险描述、影响面、接受理由与复审触发条件，作为后续重新开工的入口。

信任模型的总体背景已由 #477 交付的隧道信任模型文档覆盖：gateway→tunnel 是绝对信任
关系，升级链路的信任假设（GitHub release 同源下载）是有意设计；本文不重复论证，
只记录"在既定信任模型下仍接受的残余风险"。

## 残余风险清单

### 1. 升级包无签名校验（minisign/cosign）

- 描述：`apps/aether-tunnel/src/setup/upgrade.rs:168-208` 升级包与 `SHA256SUMS.txt`
  从同一 GitHub release 下载，校验和与二进制同源，无签名锚定；无版本白名单、
  无防降级检查、无 TLS pinning。
- 影响面：控制 release 发布渠道（或该渠道上游的 gateway 管理面）= 所有隧道节点
  root RCE。触发条件：攻击者能向隧道节点下发 `upgrade_to`（需先控制 gateway）或
  能篡改 GitHub release 资产。
- 接受理由：在 gateway→tunnel 绝对信任模型下，升级链路的信任锚与 gateway 相同；
  引入独立签名公钥会新增密钥保管与轮换负担，单维护者模式下收益不成比例。
  关联编号 ADR-0045/0050 已记录签名 provenance 与 key 轮换的逐步演进方向。
- 复审触发：威胁模型变化（对外提供托管服务、接入第三方 gateway）；隧道 agent 被
  非完全信任的 gateway 复用；启动签名 provenance（ADR-0045）完整实现时。

### 2. systemd unit 未降权（默认 root）

- 描述：`apps/aether-tunnel/src/setup/service.rs:296-318` 生成的 systemd unit 无
  `User=` 指令，进程以 root 运行。
- 影响面：隧道 agent 进程被攻破（如升级链路、解析/转发路径的内存安全缺陷）时直接
  获得节点 root，放大一切其他隧道 agent 漏洞。
- 接受理由：降权需要重排隧道可写目录（配置、缓存、升级落盘）的所有权，属部署面
  变更；当前隧道节点由部署责任人独占管理，root 与其运维模型一致。
- 复审触发：隧道 agent 面向非独占管理的通用节点分发；与其他服务共用节点部署。

### 3. 重定向 scheme 降级保留 Authorization

- 描述：`apps/aether-tunnel/src/tunnel/stream_handler.rs:783-793`
  `strip_sensitive_headers_for_redirect` 的 cross-host 判定只比较 host 与端口，
  不比较 scheme；上游返回 `Location: http://同host:443/...` 时 authorization/cookie
  经明文 HTTP 发出。
- 影响面：凭证泄露。触发条件苛刻：需 follow_redirects 开启 + 上游恶意或被盗的
  redirect 目标，且 gateway 侧 oauth/verify 流程才会开启 follow_redirects。
- 接受理由：触发需要上游配合作恶，在 gateway→tunnel 信任模型内上游即受信方；
  修复是一行比较，风险收益比不紧迫，留作纵深防御待办。
- 复审触发：follow_redirects 扩大使用范围至非受信上游；对外托管时。

### 4. 重定向重放请求体无上限

- 描述：`stream_handler.rs:605-643`（`collect_request_body_for_replay`）与
  `RequestBodyReplayState::push_chunk`（:418-428）累积请求体无字节/帧数上限。
- 影响面：隧道 agent 内存被超大重放请求体打爆（OOM）。缓解：请求体源自 gateway 侧，
  frontdoor 有 body 限额（存在 unlimited 模式），属纵深防御缺口。
- 接受理由：默认 frontdoor 限额已覆盖主要路径；隧道节点独占部署，OOM 影响局限于
  单节点且可自动恢复。
- 复审触发：frontdoor unlimited 模式成为默认；多租户共用隧道节点。

### 5. `allow_private_targets` 默认 true

- 描述：`apps/aether-tunnel/src/config.rs:253-259` 私网过滤/DNS 防 rebinding 默认
  不生效。
- 影响面：SSRF 放大。缓解：隧道目标 URL 来自 gateway 的 planner 从管理员配置的
  provider base_url 解析，普通 API 用户不能控制目标 host；admin 误配 base_url 指向
  内网/云 metadata（169.254.169.254）时才有实际暴露。README 已有警示。
- 接受理由：误配场景依赖管理员操作错误，且属管理员自助造成的自伤；默认收紧是
  一次行为变更，需迁移窗口。
- 复审触发：provider base_url 开始接受非管理员输入（如租户级自定义 provider）；
  对外托管时此项应随默认收紧一并复审。

### 6. `allowed_providers` 按 name/type 大小写不敏感匹配

- 描述：`crates/aether-scheduler-core/src/auth.rs:8-19`：允许值与 `provider_id`、
  显示名、`provider_type` 大小写不敏感相等；`allowed_providers=["openai"]` 实际放行
  所有 type=openai 的 provider（含日后新增者）。
- 影响面：授权语义放宽——admin 按 name 授权悄悄变成按 type 授权，日后新增同名 type
  的 provider 自动获得放行。
- 接受理由：测试锁定为有意设计；provider_name 仅 full admin 可改，非低权限问题；
  属文档缺失而非代码缺陷，接受"放宽语义"本身但保留文档化警示（即本记录）。
- 复审触发：allowed_providers 授权面扩展到非 full admin 可编辑的 provider 名称；
  新增 provider 的自动接入流程上线。

### 7. `send_admission`/`emergency_chain` 未接线

- 描述：`send_admission.rs:538-542`、`emergency_chain.rs:534-541` 无条件返回不可用
  错误，全仓无 gateway 调用方；2876 行为未接线代码。
- 影响面：无直接安全影响（fail-closed）；影响是能力缺失——准入预算与应急链路的
  运行时保障不存在，调度准入依赖现有 RPM/健康度机制兜底。
- 接受理由：[ADR-0044](../architecture/adr-0044-emergency-chain-domain.md) 明确记录
  这是刻意的分阶段切片；接线是功能演进，不是缺陷修复。
- 复审触发：启动 emergency chain 公共调度接线（ADR-0044 延期范围）；
  准入预算（`admission-core` body 限流）实施时。

## 复审总则

出现以下任一情况，全部 7 项重新评审：① 威胁模型变化（对外提供托管服务、接入
非受信 gateway、多租户共用节点）；② 隧道 agent 分发范围扩大（通用节点、第三方
部署）；③ 相关编号 ADR（0044/0045/0050）推进实现；④ 新增维护者导致单维护者
信任假设变化。

行为改变时按 ADR README 维护规则更新对应条目，本文标记 Superseded 并保留替代链接。
