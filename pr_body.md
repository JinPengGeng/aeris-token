Refs #526

## 方案

金额在 API 出入参边界统一改为定点表示：8 位小数字符串（如 `"12.34567800"`，1e-8 USD 微单位），与 DB 的 NUMERIC(20,8) 精度一致。内部计算仍沿用 f64（计费内核 #523 已整数化），仅序列化/反序列化层做量化，消除浮点展示/比较误差（如 `0.1+0.2=0.30000000000000004`）。

后端新增 `money_fixed` 模块（`apps/aether-gateway/src/money_fixed.rs`）：
- 出参：`format_money` / `format_money_units` 量化到 1e-8 后格式化为 8 位小数字符串。
- 入参：`deserialize_money` / `deserialize_optional_money` 接受定点字符串或 ≤8 位小数的 JSON number，超精度/非法输入直接拒绝（不静默截断）。

前端新增 `src/utils/money.ts`（`parseMoneyUnits` / `moneyToNumber` / `formatMoney` / `toMoneyString`），展示与比较基于微单位整数，避免二进制浮点误差；API 类型同步为 `string`。

## 覆盖范围

- **后端（aether-gateway）**：admin billing（plans / wallets payloads+requests / payments redeem+credit）、observability usage analytics（aggregation / attribution / cache_affinity_hit_analysis）、public support（auth_session 钱包摘要、billing、dashboard_filters、rate_limit_status、user_me_usage、wallet reads/recharge/redeem/refunds）。
- **前端**：`api/wallet|billing|admin-wallets|admin-payments|auth(BillingSummary)` 类型，与 WalletCenter、WalletOpsDrawer、WalletsManagement、BillingPlansManagement、ApiKeys、Users、Settings 及对应 fixtures/mocks。

## 破坏性变更

- 所有上述 API 的金额字段由 JSON number 改为 8 位小数字符串；入参仍兼容 number（≤8 位小数），但响应不再是 number，依赖旧类型的外部客户端需适配。
- `build_local_balance_denied_response` 的余额回显改动已被 #528 的 insufficient_quota 契约取代（该路径不再回显余额），本 PR 不再触碰。

## 测试

- `cargo test -p aether-gateway` 通过；`cargo clippy --all-targets` 无新增警告；`cargo fmt` 通过。
- 前端 `vue-tsc` 0 错误；vitest 230 文件 / 1762 用例全部通过。

## 遗留

- `handlers/proxy/mod.rs` 中 tunnel affinity 内部头 `TRUSTED_AUTH_BALANCE_HEADER` 仍为 `to_string()` 浮点格式——属节点间内部契约，未纳入本次 API 边界定点化，建议后续单独评估。
- 前端 `opsOverview` 等聚合统计仍以 number 累加（展示用途），如需精确汇总可后续改为微单位累加。
