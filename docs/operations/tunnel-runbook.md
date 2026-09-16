# Tunnel 运维入口

本页把 `aether-tunnel` 的安装、配置、升级和故障处置入口集中起来。它只引用
当前实现已经提供的命令和文档，不把 Gateway 的真实部署演练或发布审批写成
Tunnel 本地启动即可完成的验收。

## 权威资料

- [环境变量参考](../../apps/aether-tunnel/ENVIRONMENT.md) 是 clap 参数的生成清单；
  安装器读取的 `AETHER_TUNNEL_CONFIG`、`AETHER_TUNNEL_RELEASE_REPO`、
  `AETHER_TUNNEL_RELEASE_TAG` 和 `AETHER_TUNNEL_INSTALL_DIR` 在表格中单独列出。
- [Tunnel README](../../apps/aether-tunnel/README.md) 是安装和配置示例入口。
- [发布运行时策略](../issue-triage/issue-256-runtime-install-policy.md) 记录发布仓库
  的切换条件；[发布密钥轮换手册](issue-205-release-key-rotation.md) 记录签名信任集合
  和轮换/恢复边界。
- Gateway 的 [指标合同](metrics-contract.md)、[多节点部署基线](multi-node-deployment.md)
  和 [备份恢复演练](backup-restore-drill.md) 仍需按各自的真实环境验收。

## 安装源与配置

安装脚本默认从 `fawney19/Aether` 选择最新的非草稿 `tunnel-v*` release。该默认值
在 fork 发布自己的 `tunnel-v*` 制品前保持不变；不要把通用 `latest` release 当作
Tunnel 版本。需要从其他已审核的 GitHub 仓库安装时，显式设置
`AETHER_TUNNEL_RELEASE_REPO=OWNER/REPO`，脚本仍会校验仓库标识、HTTPS 下载主机和
release 资产。

运行时配置优先级为 CLI > `AETHER_TUNNEL_*` 环境变量 > TOML。修改配置后使用
`sudo aether-tunnel setup /etc/aether-tunnel/aether-tunnel.toml` 重新生成服务定义；
不要把管理 token 或加密 key 放进 shell 历史、Issue、日志或提交的示例文件。

## 启动和健康检查

安装为 systemd/OpenRC 服务后，使用以下命令确认服务状态和日志：

```sh
aether-tunnel status
sudo aether-tunnel logs
sudo aether-tunnel restart
```

systemd/OpenRC 的日志落点和权限以 README 的安装说明为准。若配置了诊断监听，
再按部署的监听地址执行健康/指标检查；没有配置 `diagnostics_bind` 时不要假设
存在对外诊断端口。Tunnel 的 heartbeat 会上报连接和错误摘要，但它不是 Gateway
的 Prometheus 抓取或告警投递验收。

## 升级与回滚

Linux/macOS 的 `sudo aether-tunnel upgrade [version]` 和 heartbeat 触发的升级都先
验证同一 release 的 `SHA256SUMS.txt.sig`，再解析已认证的 `SHA256SUMS.txt` 并校验
当前平台归档摘要；`release-provenance.json` 只提供发布审计信息，不是运行时信任根。
验证失败会保留当前二进制，不得改用未签名资产或关闭校验重试。

远程升级仍须同时满足本地 `remote_upgrade_enabled`、root 写入权限和受保护的发布
密钥配置；默认关闭。Windows 不执行进程内替换，使用 PowerShell 安装脚本完成手工
更新。heartbeat 触发的升级使用 required restart；服务重启失败时会尝试恢复上一版本。
手工 `upgrade` 使用 best-effort restart，重启失败只会记录警告并保留已替换二进制，
操作者必须先保留错误日志和当前/备份二进制，再按[密钥轮换手册](issue-205-release-key-rotation.md)
核对 key ID 和签名错误类别，禁止重写已有 release tag。

## 事件记录

每次安装、升级或回滚至少记录 UTC 时间、节点名、目标 release tag、当前/目标版本、
发布仓库、签名 key ID、归档 SHA-256、服务重启结果和日志位置。记录中不得包含
management token、tunnel encryption key、完整 URL 凭据或备份内容。

验证安装源和发布状态可使用：

```sh
gh release list --repo "${AETHER_TUNNEL_RELEASE_REPO:-fawney19/Aether}" --limit 100
rg -n 'AETHER_TUNNEL_RELEASE_REPO|tunnel-v|SHA256SUMS.txt.sig' \
  apps/aether-tunnel/install.sh apps/aether-tunnel/install.ps1 \
  apps/aether-tunnel/src/setup/upgrade.rs
```
