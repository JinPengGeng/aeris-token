# Issue #205 remote upgrade gate decision

记录日期：2026-09-12

## 复核结论

Issue #205 的远程升级风险仍成立：`apps/aether-tunnel/src/setup/upgrade.rs`
从 GitHub Release 下载制品和同源 `SHA256SUMS.txt`，客户端没有验证发布签名；
`apps/aether-tunnel/src/tunnel/heartbeat.rs` 会在收到 ACK 的 `upgrade_to` 后调用
自更新，且受管 root 服务具备替换自身二进制的权限。当前代码已经拒绝非 SemVer、
当前版本及更低版本，因此反降级不是本切片的缺口。

## 本次决定

在发布签名验证根和密钥托管方案落地前，heartbeat 触发的自动升级默认关闭。新增
`remote_upgrade_enabled` 配置（CLI：`--remote-upgrade-enabled`；环境变量：
`AETHER_TUNNEL_REMOTE_UPGRADE_ENABLED`），只有显式设为 `true` 才处理远程升级
指令。手工 `aether-tunnel upgrade` 保持原有行为，避免把本地维护操作与控制面推送
混为一谈。

这是一个可独立回滚的纵深防御切片：未配置签名验证的既有节点不会因 heartbeat
ACK 自动下载并执行未经签名锚定的 root 制品；明确 opt-in 的部署仍承担现有发布
信任边界，README 和 `.env.example` 已经写明。

## 验收覆盖

- 默认 `Config` 的 `remote_upgrade_enabled` 为 `false`。
- TOML `ConfigFile` 可往返保存 `remote_upgrade_enabled = true`，并通过环境注入
  进入运行时配置。
- heartbeat 在本地策略关闭时忽略有效版本的升级指令，不设置升级进行中的状态。
- `git diff --check` 通过；本机未安装 Rust 工具链，编译、rustfmt 和测试由 PR 的
  required CI 验证。

## 后续依赖

签名验证仍需独立设计和评审：选择 Sigstore/GitHub Artifact Attestations 或固定的
Ed25519/minisign 信任根，更新 tunnel 发布 workflow，并定义密钥轮换、撤销、离线
安装和旧版本兼容策略。在该方案合入并通过真实发布演练前，不应把本开关默认改回
`true`，也不应宣称 Issue #205 的签名缺口已完全修复。
