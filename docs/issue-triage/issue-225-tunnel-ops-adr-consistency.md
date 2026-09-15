# Issue #225：Tunnel 运维与 ADR 一致性切片

日期：2026-09-15。范围：`aether-tunnel` 的安装/升级文档、环境变量参考、
运营入口和 ADR 状态索引。该切片不改变运行时配置、协议、发布工作流或数据库行为。

## 已处理的漂移

- 安装脚本实际支持 `AETHER_TUNNEL_RELEASE_REPO`，但旧参考只列出三个安装器变量。
  `env_reference` 现在把该变量与默认仓库一起生成到 `ENVIRONMENT.md`，README 的
  已知变量检查也将它纳入同一清单。fork 尚无 `tunnel-v*` release 前，默认仍保持
  `fawney19/Aether`；切换条件见 [#256 策略](issue-256-runtime-install-policy.md)。
- README 的升级说明已与 `upgrade.rs` 和发布 workflow 对齐：先验证
  `SHA256SUMS.txt.sig`，再校验 `SHA256SUMS.txt` 中的归档摘要；
  `release-provenance.json` 只用于发布审计。远程升级仍受本地 opt-in、root 权限和
  发布密钥配置限制。
- 新增 [Tunnel 运维入口](../operations/tunnel-runbook.md)，集中链接环境、安装源、
  日志/健康检查、升级/回滚、发布密钥轮换以及 Gateway 侧的独立演练边界。
- `docs/adr/README.md` 增加编号 ADR 的交叉索引；ADR-0045 明确为核心验签已实现、
  fleet rollout 延期，ADR-0050 增加与架构索引一致的 partial implementation 状态。

## 验收边界

`git diff --check` 和新增文档的相对链接目标应通过；有 Rust 工具链时，必须运行
`cargo test -p aether-tunnel --bin aether-tunnel config::env_reference`，确认生成器
输出与 `ENVIRONMENT.md` 完全一致。该切片不宣称完成真实多节点、Prometheus
Alertmanager、生产备份恢复或发布密钥轮换演练；这些仍由 #224、#307、#223、#205
和各自运营手册验收。
