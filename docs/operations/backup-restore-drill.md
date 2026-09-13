# PostgreSQL 备份恢复演练（Issue #223）

应用备份是经过原始对象 key 认证的 JSON（`.json.zst.aes256gcm`），不是 PostgreSQL 全库快照。`aether-backup-restore` 默认只解密、验证并输出私有 JSON；显式传入 `--apply-to-empty-drill-database` 才会使用受认证的 `RecoveryBackup` 能力写入隔离数据库。

普通管理员 `/api/admin/system/data/import` 使用 `InteractiveUpload`，会将密码/API key 转为不可使用的占位信息，不能用于凭据恢复。原生 `data import --preserve-credentials` 接受另一种 JSONL 格式，也不能替代应用备份恢复。本演练不增加高权限 HTTP 入口。

## 可复核闭环

自动演练使用明确标记的合成账户、原始密码、API key、非零钱包余额和 usage aggregate。准备一个没有任何应用表的隔离 PostgreSQL 数据库，名称必须为 `aether_restore_drill_<suffix>`，然后设置 `AETHER_TEST_RESTORE_DATABASE_URL` 并运行：

   ```bash
   tools/operations/backup_restore_drill.sh
   ```

演练执行正式 bootstrap/migrations，将已知数据压缩、加密为真实应用备份，调用单独的恢复 CLI 进程，再通过 PostgreSQL 和生产读取/认证路径验证：

- 用户/API key/钱包实际行数，原密码 hash 与原 key 的恢复。
- 钱包 recharge/gift 分桶、累计金额及 usage aggregate 与源备份逐项相等。
- 原密码经 `/api/auth/login` 登录并访问 `/api/auth/me`；原 API key 请求 `/v1/models` 成功，无效 key 被拒绝。
- 再次向已恢复的非空数据库应用备份必须失败，且不输出成功 summary。

脚本仅在 exact Rust 用例实际运行 `1 passed / 0 failed / 0 ignored` 后输出 `restore_drill_verified=true fixture=synthetic`。它不会创建、删除或清空数据库；完成后保留隔离库与测试日志，合成凭据临时文件由测试删除。任何失败保留现场，不自动重新执行或清空数据。

required Rust CI 的 Gateway job 调用 `tools/ci/run_backup_restore_drill.sh`：该包装器自建仅使用私有 Unix socket 的 PostgreSQL 和独立数据库，调用同一演练脚本，结束时停止自有实例并保留证据目录。其失败会使 Gateway job 和 Rust 聚合门禁失败。Gateway HTTP 用例沿用项目的 16 MiB `RUST_MIN_STACK`。

## 对下载对象进行隔离恢复

先在另一个隔离数据库运行与备份兼容的正式迁移，并停止所有连接该库的 Gateway/worker。该 CLI 只接受 `aether_restore_drill_` 前缀的数据库，以及除正式 bootstrap 的两个默认行和 migration 记录外没有应用数据的 schema。需要独立数据库、账号和网络；名称检查不是网络隔离措施。

把数据库 URL、目标数据加密密钥和备份解密密钥分别放入 `0600` 文件，父目录设置为 `0700`。命令行只传文件路径：

```bash
aether-backup-restore \
  --input /private/drill/backup.json.zst.aes256gcm \
  --object-key '<original-complete-object-key>' \
  --output /private/drill/verified.json \
  --key-file /private/drill/backup-key \
  --apply-to-empty-drill-database \
  --database-url-file /private/drill/database-url \
  --data-key-file /private/drill/target-data-key
```

受认证的 `RestoredBackupJson` 直接传入现有恢复接口，普通 JSON 文件不能赋予恢复凭据的权限。CLI 先验证数据库身份、schema 和空库条件，并在整个 apply 期间持有专用 PostgreSQL advisory lock。该锁仅协调本工具；运维必须保持目标应用停止。导入错误或非空错误集合使进程失败，不回显可能含敏感内容的导入响应。

`database_applied=true` 仅表示认证恢复入口返回成功；`acceptance_verified=false` 明确表示尚未完成该下载对象的业务对账。需要根据真实备份中的用户、密钥、钱包、分组和聚合范围建立源/目标清单并逐项比较，再验证原密码登录和原 key。不能用合成演练通过替代某个生产备份的验收。

应用 data 备份包含配置、非管理员用户、用户/API key 钱包、分组以及 usage aggregates；不包含所有原始 usage 行、完整 wallet ledger、管理员账户或完整数据库运行状态。不能据此声称已恢复每一笔原始请求或历史账本；有该需求时应另外配置并演练 PostgreSQL 原生备份/PITR。

## RPO、RTO 与部分失败

RPO 是备份 `exported_at` 到故障点的时间差；生产目标由部署者填写并告警。RTO 从恢复工作开始，直到迁移、导入、源/目标对账、登录和 API key 验收全部完成。合成演练报告其测试耗时，不代表生产 RTO；不能用解密耗时代替 RTO。

导入由系统互斥租约保护，但跨表步骤可能在连接中断或租约丢失时部分提交。失败后保持目标停止流量，保留日志和隔离库，另外准备干净隔离库并从同一加密对象及原始 object key 重试。不要对未知状态库盲目重复导入。该 CLI 不用于原地生产覆盖恢复。

## 证据留存

保留 object key、cipher SHA-256、exported_at/key_id、迁移版本、演练开始/结束 UTC、安全 summary、测试日志及业务对账结果。不要提交备份 JSON、密码、API key、加密密钥或含凭据的响应。

## 独立评审后的决定

原脚本因使用 InteractiveUpload 和仅检查 JSON 字段真值而被拒绝；空对象/数组不能证明恢复成功。本实现改为真实认证 CLI、空隔离库约束和可失败的实质对账，默认解密行为保留。自动化覆盖合成的完整 data 容器中的用户/密钥/钱包和 daily aggregate；不声称已经演练生产备份、所有 provider 凭据或原始账本。父 #223 保持开放。

本地验证（2026-09-13，Rust 1.95 / PostgreSQL 17.11）：完整 CI 包装器实际运行 `1 passed / 0 failed / 0 ignored`，测试耗时 3.02 秒，脚本含测试构建阶段耗时 46 秒，二进制构建另计。验收包括篡改密文拒绝且数据库未改、真实认证 apply、钱包和完整 fixture aggregate 相等、原密码登录及会话验证、原 key `/v1/models` 成功、错误 key 拒绝、占用数据库拒绝重复 apply。ShellCheck、actionlint 与 diff 检查通过。

演练开发时发现测试服务器未附带正式 `ConnectInfo`，HTTP 登录返回 500；改用已有 `tests::start_server`。随后真实 HTTP 分支触发 libtest 默认线程栈不足；采用 CI 已有的 16 MiB 设置后完整用例通过。两次失败均未输出演练成功，保留隔离库，没有修改认证或生产计费行为来迁就测试。
