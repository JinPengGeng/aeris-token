# Issue 300：资金预留前钱包被禁用

## 结论

公共图片请求在认证准入后才进行 durable pending 和资金预留。钱包状态可能在这两个阶段之间被管理员撤销。数据层会以 `WalletUnavailable` 拒绝 reserve；该结果表示没有 attempt reservation、没有上游发送，也不表示发生了模糊的数据库提交失败。

网关现已将这个确定的资金拒绝写成零费用的父 `failed` 终态。父终态写入沿用现有生命周期事件；如果已有 attempt，拒绝事件只引用最近 attempt 的持久身份，不覆盖该 attempt 的 Charged、NoCharge 或 Unknown 事实。Unknown hold 仍保留，迟到收费仍可独立完成。

## 验收

- `live_public_image_wallet_disabled_after_auth_finishes_failed_parent`：在 durable pending 插入后由数据库触发器禁用钱包；断言返回错误、零上游请求、零资金/配额 reservation，父请求 `failed` 且零费用。
- `live_gateway_image_wallet_unavailable_preserves_prior_attempt_facts`：先完成一次 attempt，再禁用钱包并尝试下一候选；分别覆盖 Charged 与 Unknown，断言第二次没有上游发送，父状态为 failed，第一次事实、实际费用和 Unknown hold 不被覆盖。

主线程先保留旧分支运行公共入口回归，真实复现父记录为
`("pending", 0, false)`，而预期为 `("failed", 0, true)`。
恢复专门的 `WalletUnavailable` 处理后，两项新增回归以及原有资金专项共
20 项全部通过，零失败、零 ignored；其中 16 项使用真实 PostgreSQL/HTTP。
新增目标已接入 required Gateway live harness，等待最新提交的 Hosted 验证。

本地使用隔离数据库 `aeris_quota_scope_startup`，在现有任务专属 PostgreSQL
17.11 实例上新建，先执行当前 bootstrap 加增量迁移。保留失败场景的测试数据，
未修改原 Gateway/quota 测试库。日志：
`/private/tmp/aeris-wallet-unavailable-red-20260913.log` 与
`/private/tmp/aeris-wallet-unavailable-green-20260913.log`。

rustfmt、ShellCheck、`bash -n` 和 diff 检查均通过。本修正不把未知数据库错误
归类为 WalletUnavailable，未知错误仍走原有恢复路径。

## 相关决策/相关文档

- 本决策属于 [#300](https://github.com/JinPengGeng/aeris-token/issues/300) 资金生命周期的组成部分，与 #206 恢复路径相关。
- Gateway 侧 attempt 资金接线与验收边界见[钱包不可用后的 attempt 资金接线](issue-300-gateway-attempt-funds.md)。
- 完整的 attempt 子记录资金生命周期决策见 [issue-300-gateway-lifecycle.md](issue-300-gateway-lifecycle.md)。
