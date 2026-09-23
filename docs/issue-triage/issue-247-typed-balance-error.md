# Issue #247：余额不足类型化错误体（方案 A）

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
