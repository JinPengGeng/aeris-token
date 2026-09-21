# 通知项"配置幻觉"核对（2026-09-21，Refs #247）

针对 Issue #247 提出的"通知体系可配置但从不触发"，对全部默认通知项逐项核对 producer 与触发点。
结论：6 个默认通知项中 5 个已有 producer，1 个（`provider_pool_abnormal`）为断链项，本次已补最小接线。

## 核对表

| 通知项 key | 配置入口 | Producer | 触发点 | 结论 |
| --- | --- | --- | --- | --- |
| `provider_quota_alert`（号池额度不足） | `important_notification.rs` 默认项 + `aether-admin` 系统配置默认值 | `maintenance/runtime/provider_quota_alert.rs`（`perform_provider_quota_alert_once`，按 30s 间隔 + 24h 冷却去重） | 号池余额低于阈值（默认 $10）时全局通道告警 | ✅ 已接通 |
| `provider_pool_abnormal`（号池异常） | `aether-admin` 系统配置默认值（gateway 默认项此前缺失） | **此前无任何 producer**；本次新增 `maintenance/runtime/provider_checkin.rs::maybe_notify_provider_pool_abnormal`，并补入 gateway 默认项 | Provider 定时签到失败（`perform_provider_checkin_once` Failed 分支），每 provider 24h 冷却 | ✅ 本次补齐 |
| `user_balance_low`（用户余额不足） | `important_notification.rs` 默认项（默认阈值 $10，可配置 `module.important_notification.user_balance_low_threshold`） | `important_notification.rs::maybe_send_user_low_balance_notification`（余额恢复后释放去重） | `state/runtime/wallet/reads.rs::schedule_low_balance_notification`（钱包读取路径异步触发），尊重用户 `usage_alerts` 偏好 | ✅ 已接通 |
| `user_refund_status`（退款状态更新） | `important_notification.rs` 默认项 + `aether-admin` 默认值 | `maintenance/runtime/refund_notifications.rs`（持久化 outbox worker，5s 轮询，重试/跳过分流） | 退款进入终态（succeeded/failed）后向用户邮箱投递 | ✅ 已接通 |
| `user_recharge_recovery`（充值后历史欠费处理结果） | `important_notification.rs` 默认项 | `maintenance/runtime/recharge_recovery.rs` | 充值后历史欠费追扣结果通知用户 | ✅ 已接通 |
| `recharge_recovery_review`（历史欠费追扣待处理） | `important_notification.rs` 默认项 | `maintenance/runtime/recharge_recovery.rs` | 追扣进入待人工审核时通知管理员 | ✅ 已接通 |

## 说明

- 用户偏好 `usage_alerts` 为零消费的问题随 `user_balance_low` producer 落地已消解：低余额邮件在 `usage_alerts=false` 时跳过（`maybe_send_user_low_balance_notification` 内显式检查）。
- 余额不足误报 429 的核对：鉴权层 `BalanceDenied` → `api/response.rs::build_local_balance_denied_response`（`insufficient_quota` / `credit_balance_exhausted`）；请求中途余额耗尽由 `execution_runtime/submission.rs::classify_local_sync_error_kind` 归类为 `QuotaExhausted`（billing 分类优先于 429 猜测）。未发现仍误报 429 的路径，本次未改动。
- `provider_pool_abnormal` 接线为最小实现：仅在 provider 签到失败时触发，按 provider 维度 24 小时冷却，通知内容使用 admin 默认模板变量 `{provider_name}`；禁用通知模块或该通知项时不投递。
