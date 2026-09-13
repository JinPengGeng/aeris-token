# 恢复的原子性与失败语义

状态：Accepted。日期：2026-09-13。源码基线与维护规则见 [ADR 索引](README.md)。
关联：Issue #235、#223、#255。

## 背景与决定

恢复同时涉及配置、用户、凭据、钱包与聚合记录。它们通过独立仓储写接口访问，
聚合导入没有可跨全部接口共享的数据库事务。因此接受“预校验、互斥、检查点与补偿”
作为当前故障处理方式，不承诺全库 ACID 原子恢复。

只有 [restore_backup_json](../../apps/aether-gateway/src/backup/executor.rs)
成功验证加密 envelope、原始完整 object key、压缩大小和 JSON 元数据，才能创建
`RestoredBackupJson` 及其恢复权限。[apply_restored_backup](../../apps/aether-gateway/src/backup/mod.rs)
再次检查认证 scope，随后使用共享系统导入租约调用对应恢复入口。普通管理员上传使用
`InteractiveUpload`；它不能通过提交普通 JSON 获得恢复原密码/API key 的权限。

[聚合导入](../../apps/aether-gateway/src/handlers/admin/request/system/import.rs) 的
`import_admin_system_data_with_mode` 先预校验配置与用户部分，再采集两个检查点，
按配置、用户阶段写入并记录 mutation journal。已处理的阶段错误触发相应补偿；
恢复检查点可在内存中暂存原凭据，用于恢复被覆盖的值，交互上传检查点保持脱敏。
新增对象的清理和已有钱包等状态的恢复使用 journal 及匹配条件，失败不能被吞掉。
这是一组应用级补偿动作；它们本身也可能失败。

[租约实现](../../apps/aether-gateway/src/handlers/admin/system/import_lock.rs) 将操作与
租约丢失竞争；丢失时取消操作并返回 `Lost`。取消或连接中断可能发生在一次仓储写入
提交之后，不能保证执行后续补偿。互斥租约只协调接入该机制的导入/恢复，不能将其他
业务写入一并暂停。独立 PostgreSQL advisory lock 也只协调恢复 CLI。

## 故障与验收边界

| 观察结果 | 当前语义与操作 |
| --- | --- |
| 密文认证、scope 或预校验失败 | 拒绝该入口后续 apply；认证成功不是数据库恢复成功 |
| 写入阶段报错且补偿成功 | 返回原失败，不输出恢复成功 |
| 补偿失败、租约丢失、取消或连接故障 | 目标可能部分写入；保持停流，保留隔离库与日志供对账 |
| CLI 输出 `database_applied=true` | 认证 apply 返回成功且无嵌套导入错误；`acceptance_verified=false` 仍要求业务对账 |

[aether-backup-restore](../../apps/aether-gateway/src/bin/aether-backup-restore.rs)
默认只输出私有的已验证 JSON。显式 apply 仅接受带 `aether_restore_drill_` 前缀且
满足正式 schema/空应用数据检查的隔离 PostgreSQL 数据库；不会清空未知目标。
要求停止目标 Gateway/worker、保留失败现场，并在另一干净隔离库重试。
完整命令、证据清单与 RPO/RTO 口径见 [恢复演练手册](../operations/backup-restore-drill.md)。

## 取舍、兼容与回滚

复用已有仓储和导入校验，使认证恢复不需要另开高权限 HTTP 入口；代价是检查点内存、
补偿复杂度，以及崩溃时仍需要人工对账。全局长事务或完整数据库快照恢复是不同方案，
当前接口不能用“补偿”替代它们的隔离保证。

备份 envelope v2 按 key ID 选择解密候选；v1 只尝试获准用于 legacy 的候选密钥。
认证解密兼容与 JSON schema/import version 校验是两层限制，不能只凭 envelope 可解密
就承诺旧应用能导入。保留原始 object key、历史解密密钥与匹配的软件/迁移版本。
回滚代码不会撤销已写数据库；失败恢复的重试使用新的隔离目标，生产切换须在对账后
由部署责任人执行。

## 可复核证据与未完成范围

- [认证与大小限制测试](../../apps/aether-gateway/src/backup/executor.rs)：
  `restore_keeps_v1_compatibility_and_tries_only_legacy_candidates`、
  `restore_rejects_zstd_output_over_limit`。
- [恢复入口测试](../../apps/aether-gateway/src/backup/mod.rs)：
  `authenticated_backup_cannot_be_applied_to_a_different_scope`、
  `authenticated_backup_apply_uses_the_shared_system_import_lock`。
- [租约测试](../../apps/aether-gateway/src/handlers/admin/system/import_lock.rs)：
  `admin_system_import_operation_is_cancelled_when_lease_is_lost`；
  [导入测试](../../apps/aether-gateway/src/tests/control/admin/system_import.rs)：
  `gateway_prevalidates_aggregate_user_data_before_config_mutation`、
  `authenticated_recovery_restores_password_and_api_key_login_material`。
- [真实 PostgreSQL 合成演练](../../apps/aether-gateway/src/backup/tests/restore_drill.rs)：
  `live_authenticated_restore_cli_preserves_credentials_wallets_and_aggregates`，
  执行入口为 [CI 包装器](../../tools/ci/run_backup_restore_drill.sh)。

本次只读取上述代码和断言，没有重跑演练。既有演练的执行结果见手册；它验证合成
账户的认证、钱包与 aggregate，不覆盖每个生产备份、所有平台、完整原始账本、活动
资金预留或全库 PITR。全库灾备、真实部署 RPO/RTO、全部补偿故障矩阵仍是独立验收。
维护责任由恢复/管理入口及数据仓储的现有评审角色共同承担，现场验收由部署责任人承担。
