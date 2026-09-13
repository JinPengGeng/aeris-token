# PostgreSQL 备份恢复演练（Issue #223）

应用备份是经过对象 key 认证的 JSON（`.json.zst.aes256gcm`），不是 `pg_dump`。离线工具只解密和校验；真正写库必须在维护窗口调用管理员 data import。恢复前应准备一台与生产隔离的 PostgreSQL（独立数据库、账号和网络），并确认迁移已完成。

## 可复核闭环

1. 从对象存储下载完整对象，使用原始 object key 执行 `aether-backup-restore`，保留 stdout 中的 `cipher_sha256`、`key_id`、`exported_at`。解密输出和 key 文件权限应为 `0600`。
2. 设置 `DATABASE_URL` 为隔离库、`GATEWAY_URL` 为连接该库的临时网关、`ADMIN_TOKEN` 为短期管理员 token、`RESTORED_JSON` 为已校验 JSON，运行：

   ```bash
   tools/operations/backup_restore_drill.sh
   ```

   脚本在导入前后查询 `users`、`auth_api_keys`、`wallets`、`usage` 行数及 usage 成本总额；随后重新导出并用 jq 确认用户、独立 API key 与 usage aggregate 三个投影存在。脚本不会创建或删除数据库，也不会伪造登录或 key。

3. 人工完成三项业务验收并记录响应和时间：使用导出用户的密码登录；用导出 API key 请求一个无副作用的 `/v1/models`（或受控 provider probe）；查询同一 usage 行和 wallet ledger，确认 request id、tokens、cost、余额扣减与导出快照一致。API key 明文只在临时演练环境生成，日志不得保存。

## RPO、RTO 与部分失败

RPO 是备份 `exported_at` 到故障点的时间差；生产目标由部署者填写并告警（建议每日备份将目标写成不超过 24 小时）。RTO 从隔离网关可接受流量前开始计时，直到迁移、导入、登录、API key 和账务三项验收全部通过；每次演练应记录秒数，不能用解密耗时代替 RTO。

导入由系统互斥租约保护，但跨表步骤可能在连接中断或租约丢失时部分提交。出现 `restore ... may have partially applied changes` 时立即停止流量，保留响应、网关日志和数据库快照，重新创建干净隔离库再从同一已校验 JSON 重试；不要在未知状态的生产库盲目重复导入。失败项按导入响应中的 `stats` 和上述 SQL 对账，差异归零后才算恢复成功。

## 证据留存

保留 object key、cipher SHA-256、exported_at/key_id、迁移版本、演练开始/结束 UTC、脚本 stdout、导入 HTTP 响应（脱敏）及三项业务验收结果。不要提交备份 JSON、密码、API key、加密密钥或含凭据的响应。
