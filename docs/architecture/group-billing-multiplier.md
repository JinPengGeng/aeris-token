# 用户组计费倍率与倍率轴语义(fork #553)

## 两条独立的倍率轴

结算金额 = 标价金额 × 供应商格式权重(provider format weight)× 计费倍率(billing multiplier)。两轴相互独立、结算时连乘。

### 供应商格式权重轴(provider_api_keys.rate_multipliers)

- 上游已于 `11c5884d` 移除管理页面入口,默认 1 倍率(不生效)。
- 本 fork 跟随上游:不启用该功能,不提供全局格式权重表,不恢复 UI。
- 后端保留既有查询/快照结构(`pricing.rs::rate_multiplier_for_api_format`),仅作为兼容层存在。

### 计费倍率轴(billing multiplier,本 fork 扩展)

单层覆盖语义(方案 A),key 与组之间**不连乘**:

1. key 级显式配置(api_keys.billing_multiplier,非默认值 1.0)生效;
2. 否则用户组级 `billing_multiplier` 生效;
3. 都无则为 1.0(原价)。

一个用户属于多个组时,取配置了倍率的组中 `priority` 最高者(并列按组名升序)。

- 精度:定点 4 位小数(内部整数万分位),合法范围 0.0001~9999.9999;`0` 为显式免费,是合法值。
- 快照:倍率在 quote(入场预冻结)时刻冻结进 `settlement_snapshot` / 图片授权 quote,管理员事后改价或改倍率不回溯在途与已结请求;结算一律从快照取值。
- key 级存储:`api_keys.billing_multiplier numeric(10,6)` 为上游所有列,本 fork 不改其 schema;读写统一按 4 位小数归一(fork 迁移 `20260928000000_round_api_key_billing_multiplier` 对存量值做一次舍入)。
- 组级存储:`user_groups` 表为上游所有,不可 ALTER;组倍率放在 fork 自有的 system config JSON 配置位 `user_group_billing_multipliers`(`{group_id: number}`),随 admin 组创建/更新/删除维护。
- 风控钳制:admin 写入(key 与组共用)时钳制到区间边界,默认 0.01~100;上下限为后台系统设置项(`billing_multiplier_clamp_min` / `billing_multiplier_clamp_max`,admin 可改、走审计);显式 0(免费)不受下限钳制,放行。
- API 兼容:响应输出数字(f64 JSON number),内部定点;反序列化时对 >4 位的输入舍入、非法值拒绝。

## 毛利报表「标价收入」

毛利报表(margin report)在 `RequestAttemptBilledUsage` 上新增 `list_price_cost_units`:结算时刻用已冻结的两个倍率快照把实际收入除回标价(未乘任何倍率的应收金额),与现有实际收入列并存。报表行新增 `list_price_revenue` 列,前端 MarginReport 同步加列。存量行(无该字段)按实际收入回填。

## 审计

- 组创建/更新/删除沿用 `admin_user_group_created/updated/deleted` 事件;
- 载荷中带 `billing_multiplier` 时额外附着 `admin_user_group_billing_multiplier_updated`;
- 钳制区间修改走系统设置更新审计(`admin_system_settings_updated`);
- 事件清单同步维护于 `docs/issue-triage/issue-255-admin-mutation-inventory.txt`。
