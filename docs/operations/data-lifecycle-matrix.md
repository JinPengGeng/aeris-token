# 数据生命周期矩阵（Data Lifecycle Matrix）

日期：2026-09-21
关联：Issue #223（故障恢复与数据生命周期）

本文档汇总 Aeris 各数据面的保留期、清理任务与恢复入口，作为运营时的单一索引。

## 表级生命周期矩阵

| 数据面 | 存储 | 默认保留期 | 清理任务 | 配置键（system_configs） | 恢复入口 |
|---|---|---|---|---|---|
| 用量明细（detail） | Postgres `usage_audits` 等 | 7 天 | `maintenance.usage.cleanup`（每日 03:00） | `detail_log_retention_days` | 备份 restore（见 `backup-restore-drill.md`） |
| 用量压缩层 | Postgres | 30 天 | 同上 | `compressed_log_retention_days` | 同上 |
| 用量头信息 | Postgres | 90 天 | 同上 | `header_retention_days` | 同上 |
| 用量日志（最外层） | Postgres | 365 天 | 同上 | `log_retention_days` | 同上 |
| 审计日志 | Postgres `audit_logs` | 30 天 | `maintenance.audit.cleanup` | `audit_log_retention_days` | 备份 restore |
| 代理节点指标 1m | Postgres | 30 天 | `maintenance.proxy.node.metrics.cleanup`（每日 02:10） | `proxy_node_metrics_1m_retention_days` | 无需恢复（可重建） |
| 代理节点指标 1h | Postgres | 180 天 | 同上 | `proxy_node_metrics_1h_retention_days` | 无需恢复（可重建） |
| stats 小时聚合（5 张 `stats_hourly*`） | Postgres | 180 天 | `maintenance.data.lifecycle.cleanup`（每日 03:40） | `stats_hourly_retention_days` | 从 usage 事实重跑小时聚合（catch-up burst） |
| stats 天聚合（19 张 `stats_daily*`/`stats_user_daily*`） | Postgres | 730 天 | 同上 | `stats_daily_retention_days` | 从 usage 事实重跑日聚合 |
| stats 汇总（`stats_summary`/`stats_user_summary`） | Postgres | 无限（累计快照） | 不清理 | — | 从 usage 事实重建（admin rebuild） |
| `video_tasks` 终态记录 | Postgres | 30 天 | `maintenance.data.lifecycle.cleanup` | `video_task_terminal_retention_days` | 无（终态记录仅作审计） |
| 用量事件主 stream | Redis `usage:events` | 200k 条（maxlen） | XADD 近似裁剪 | `AETHER_GATEWAY_USAGE_QUEUE_STREAM_MAXLEN` | DLQ redrive / consumer group reclaim |
| 用量死信（DLQ） | Redis `usage:events:dlq` | 50k 条且 14 天 | worker 周期修剪（保留期到龄丢弃+指标+告警） | `AETHER_GATEWAY_USAGE_QUEUE_DLQ_MAXLEN`、`AETHER_GATEWAY_USAGE_QUEUE_DLQ_RETENTION_SECS` | `GET /api/admin/usage/dlq`、`POST /api/admin/usage/dlq/{id}/redrive` |

## 关键语义

- **幂等性**：所有清理任务按批处理、可安全重跑；stats 聚合与 video_tasks 清理以桶/终态时间为界，重跑删除 0 行。
- **删除为有损**：DLQ 保留期修剪、stats/video_tasks 清理均为物理删除。需要留存时先走 S3 备份导出。
- **enable_auto_cleanup**：置 `false` 可停用自动清理（usage/proxy/stats/video 清理统一尊重该开关）。
- **批次上限**：数据生命周期清理每次运行最多 32 批 × `cleanup_batch_size`（默认 5000），长期未清理后首跑会自动分日摊销。
- **destructive SQL lint**：迁移/回填中的 DROP/TRUNCATE/无 WHERE DELETE 必须带 `-- destructive-sql: allow <reason>` 豁免注释，否则 CI 失败。

## 恢复入口索引

- 备份与恢复演练：`docs/operations/backup-restore-drill.md`
- 原生财务账本恢复：`docs/operations/native-financial-ledger-restore.md`
- DLQ 重放：`POST /api/admin/usage/dlq/{id}/redrive`（幂等，重复请求返回 `already_redriven`）
- 备份任务失败重试：`system.s3.backup` 已配置有界重试（最多 3 次尝试，30s×attempt 退避后自动重新入队）
