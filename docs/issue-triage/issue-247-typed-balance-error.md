# Issue #247：余额不足类型化错误体（方案 A）

> **演进（2026-09-23）：本方案 A（泛化 `balance_exceeded` + `details.remaining` 回显
> 余额）已被统一配额不足契约取代，见
> [error-contract.md](../api/error-contract.md) 与 issue #343/#247 最新决策。**
> 泛化路径与 OpenAI/Claude 一样收敛为 `429/insufficient_quota` 信封且不回显余额；
> `balance_exceeded` 仅保留为上游入站错误的分类 marker。以下为原始记录。

日期：2026-09-23
决定：维护者已拍板（issue #247 方案 A）——余额不足响应保持 HTTP 429 不变，
泛化错误体升级为上游同款类型化 `balance_exceeded` 错误体。

## 改动范围

`apps/aether-gateway/src/api/response.rs::build_local_balance_denied_response`
的 generic（未知客户端格式）错误体，逐字对齐上游 fawney19/Aether
（HEAD ec95989）的 `build_local_balance_denied_response`：

- `error.type = "balance_exceeded"`；`error.message = "余额不足（剩余: $X.XX）"`
  （无 remaining 时为 `"余额不足"`）；`error.details = {"balance_type":"USD","remaining":<number|null>}`。
- 保持 429、无 `Retry-After`（充值前重试无意义）。
- `remaining` 沿既有判定链透传：`control/auth/gate.rs` 的
  `BalanceDenied { remaining }` → `api/response.rs`，无链路改动。

## 不变的部分（fork 刻意保留）

OpenAI / Claude 客户端格式的错误体 contract 不变（issue #343 决策仍有效）：

- OpenAI 家族：429 / `insufficient_quota` / `credit_balance_exceeded`，
  message `Insufficient quota`，不回显余额。
- Claude Messages：402 / `billing_error` / `balance_exceeded`，message
  `Insufficient quota`，不回显余额。

上游实现对客户端格式请求同样回退到泛化体；fork 的格式化 contract 是
OpenAI/Claude SDK 兼容层的一部分，不属于本次对齐范围。若未来与上游全面
sync 需要收敛，应先在 issue 里显式废弃 #343 contract。

## 兼容性

- 旧 generic 体 `{"type":"insufficient_quota","code":"credit_balance_exhausted","message":"Insufficient quota"}`
  被 `balance_exceeded` 体取代；generic 路径主要面向非协议端点（如 dashboard
  类入口），消费方应按 `error.type` 分支。
- `execution_runtime/submission.rs::classify_local_sync_error_kind` 早已把
  `balance_exceeded` 识别为 QuotaExhausted，协议内路径不受影响。
- 前端 `features/usage` 错误展示按 domain 通用渲染，无按旧 type/code 的
  硬匹配，无需改动。

## 验收

`apps/aether-gateway/src/api/response.rs` 单元测试：
`generic_balance_denial_uses_typed_balance_exceeded_contract`（429 + 类型化体
+ details.remaining + 无 Retry-After）、
`generic_balance_denial_without_remaining_omits_amount`、
`openai_balance_denial_uses_insufficient_quota_contract`（#343 contract 不回退）。

## 演进记录（2026-09-23）：从 balance_exceeded 到统一 insufficient_quota

决策（用户定案）：余额/配额不足的全路径响应统一为：

- OpenAI 路由族 + 泛化路径 + 图片预授权：`429` + OpenAI 信封
  `{"error":{"message":"Insufficient quota","type":"insufficient_quota","param":null,"code":"insufficient_quota"}}`，
  不回显余额。
- Claude 路径：`403` + `{"type":"error","error":{"type":"insufficient_quota","message":"Insufficient quota"}}`
  （无 `code` 字段），不回显余额。

理由：

1. 对齐 OpenAI 官方错误码：`insufficient_quota` 是官方 code，
   `credit_balance_exhausted` 是社区网关的私有扩展。
2. 对齐 Anthropic 官方语义：欠费是账户状态错误，不是 429 速率窗口，
   不可重试；sub2api/new-api 对 Claude 侧均用 403 + Anthropic 信封。
3. 不回显余额：余额属于账户隐私，`details.remaining` 会泄露钱包快照；
   不同格式统一也降低客户端分支成本。
4. 与上游 `balance_exceeded` 的收敛保持刻意分叉：`balance_exceeded` 仍被
   `is_quota_exhausted_error` 识别用于入站上游错误分类，但对外响应统一重建。

破坏性变更：Claude 402→403；泛化路径不再回显余额且 type/code 改变；
OpenAI code 从 `credit_balance_exhausted` 改为 `insufficient_quota`。
