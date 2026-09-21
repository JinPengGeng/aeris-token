# ADR: 自研组件评估登记

- 日期：2026-09-21
- 状态：已登记（纯文档，无代码变更）
- Refs: #226（第四轮评审 · 技术选型：自研组件评估）

对 issue 点名的自研组件及同批次识别的候选逐一登记"现状 — 风险 — 候选替换 — 优先级"。本登记不含代码改动；标注"成立"的结论维持现状。

## 登记项

### 1. python_fernet（crates/aether-crypto）

- **现状**：422 行，8 测试。为兼容 Python 遗留 fernet 密文而自研；crates.io `fernet` crate 维护停滞，自研理由成立。含篡改拒绝、回归向量、派生缓存上限。
- **风险**：低。密码学实现经审查要点覆盖，且无活跃上游可换。
- **候选替换**：无（上游停滞）。
- **优先级 / 结论**：**成立，维持自研**，无需动作。✅

### 2. metrics 渲染器（crates/aether-runtime/base/src/metrics.rs）

- **现状**：186 行，3 测试，零依赖。仅 Counter/Gauge、value 为 u64。
- **风险**：中。无 histogram/f64，延迟分布无法表达，已成观测能力天花板（与"计费零指标"互为佐证）。
- **候选替换**：`prometheus` crate；或自研扩展（见 metrics-histogram-evaluation.md，推荐后者）。
- **优先级 / 结论**：**保留自研 + 扩展 histogram**，独立 issue 排期。⚠️ → 方案已定

### 3. formula_engine 计费表达式引擎（crates/aether-billing）

- **现状**：934 行，仅 6 测试。白名单递归下降解析器，方向正确。
- **风险**：中高。除零/幂溢出边界测试不足；`quantize_cost` 对非有限值直接透传（`precision.rs`），防线全在调用侧分散的 `is_finite` 检查——新增调用点漏检查即计费漏洞。
- **候选替换**：无直接可换的计费表达式 crate（业务语义特异）；成熟方向是"把非有限值拒绝下沉到引擎或 quantize 层"+ 补边界测试。
- **优先级 / 结论**：**成立但需加固**，列为批次 K 之后的高优先级独立 issue（不在本受控切片内改代码）。⚠️

### 4. 手写 hyper h2c client（execution_runtime/transport.rs:2019 一带）

- **现状**：绕过 reqwest/wreq 直接以 hyper 手写的 h2c（明文 HTTP/2）上游客户端，HTTP 客户端实为三套并存（reqwest 1,403 处 + wreq 49 处 + 手写 hyper）。
- **风险**：中。三套客户端并存扩大 TLS/连接池行为差异与审计面；h2c 为少数上游专有路径，自研理由（需要裸 h2c）部分成立。
- **候选替换**：hyper/hyper-util 官方 h2c 路径（随 hyper-util 成熟逐步收敛）；或评估 reqwest 的 `http2_prior_knowledge` 后下线手写版。
- **优先级 / 结论**：**观察项**。低优先级；先随 wreq 策略收敛（见 wreq-exit-strategy.md），再评估是否合并到 reqwest 单一传输。

### 5. tunnel 协议栈（apps/aether-tunnel + crates/aether-gateway/tunnel）

- **现状**：自研隧道协议，承载网关↔tunnel 数据面；版本兼容策略已有文档（docs/tunnel-version-compatibility.md）。
- **风险**：中。协议自持意味着互操作、升级、兼容矩阵均为自有成本；但该面是核心差异化能力（跨网穿透+指纹出口），换成熟方案（如 wstunnel、rathole）会丢失与计费/调度的内建集成。
- **候选替换**：wstunnel / rathole / frp 类——均不满足"与 gateway 控制面内建集成 + 指纹出口"的组合需求。
- **优先级 / 结论**：**维持自研**，升级纪律按 tunnel-version-compatibility.md 执行；每半年复查一次替代方案生态。✅（附带：批次内已确认 tunnel 依赖面冻结，不再改动）

### 6. 调度器（crates/aether-scheduler-core）

- **现状**：自研任务/容量调度核心，与 admission-core、dispatch-core 分层。
- **风险**：低中。调度逻辑与业务配额模型深度耦合，外部方案（如 Kubernetes 调度器、Temporal）无法直接承载租户级配额语义。
- **候选替换**：Temporal / cadence 类工作流引擎仅覆盖"任务编排"子集，不覆盖准入配额；不建议替换编排以外的部分。
- **优先级 / 结论**：**维持自研**；如未来出现"租户配额+优先级+抢占"的开源成熟实现再评估。✅

## 汇总

| 组件 | 结论 | 优先级 |
|---|---|---|
| python_fernet | 成立，维持 | — |
| metrics 渲染器 | 保留自研，扩展 histogram | 中（方案已定，独立 issue） |
| formula_engine | 成立但需加固（非有限值下沉 + 边界测试） | 高（独立 issue） |
| 手写 h2c client | 观察项，随传输收敛再评估 | 低 |
| tunnel 协议栈 | 维持自研，半年复查 | — |
| scheduler-core | 维持自研 | — |
