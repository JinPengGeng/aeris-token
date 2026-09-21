# ADR: metrics 渲染器 histogram 能力评估

- 日期：2026-09-21
- 状态：已评估（纯文档，无代码变更）
- Refs: #226（第四轮评审：metrics 仅 Counter/Gauge、value 为 u64，无 histogram/f64）

## 现状

`crates/aether-runtime/base/src/metrics.rs`（186 行）为自研零依赖 Prometheus 文本渲染器：

- `MetricKind` 仅 `Counter` / `Gauge` 两型；`MetricSample.value: u64`。
- 指标为进程级 `AtomicU64` 静态量（billing 失败计数、fail-open 计数等），渲染端 `render_prometheus_text` 负责 `# TYPE` 声明与命名空间前缀。
- 标签集合固定（设计注释明确：不可信请求不能创建新时间序列），基数可控。
- 全仓消费点：`aether-runtime/state`、`aether-runtime/base`（tracing writer、concurrency、queue）、`aether-testing/loadtools` 等。

## 哪些 SLI 需要 histogram

当前 counter/gauge 无法表达**延迟分布**，下列 SLI 是真实需求（按优先级）：

| SLI | 现状 | histogram 后获得 |
|---|---|---|
| 网关请求端到端延迟（p50/p95/p99） | 无（只有计数） | 分位数、SLO 违规率 |
| 上游 LLM 首 token 延迟（TTFT） | 无 | 分位数，upstream 慢判定 |
| WS 代理会话时长 / 上游建连耗时 | 无 | 建连失败与慢连分层 |
| 计费结算耗时（settlement path） | 无 | 结算路径退化告警 |
| 队列停留时长（queue） | gauge 瞬时值 | 排队延迟分布 |

## 设计建议

- **bucket 设计**：每个 histogram 用稀疏对数桶，10–12 个桶（如 5ms/10ms/25ms/50ms/100ms/250ms/500ms/1s/2.5s/5s/+Inf），覆盖 5ms–5s 的网关/上游延迟域；+Inf 桶即计数桶，天然兼容现有 counter 语义。
- **标签纪律沿用现状**：histogram 系列同样只允许固定标签集，不接受请求路径等高开销标签；按当前消费点估计新增时间序列 < 100 条。
- **渲染扩展**：`MetricKind` 增加 `Histogram { buckets: &'static [f64], counts: Vec<u64> }`、`value` 增加 f64 支持（或新增 `MetricSampleF64`），`render_prometheus_text` 追加 `_bucket{le="..."}` 行与 `_sum`/`_count` 行。渲染器已有 family 去重逻辑可复用。

## 引入成本

- **基数/存储**：按上述桶数与标签纪律，新增序列约 < 100 条 × 12 桶；相比现状（数十条 counter/gauge）量级不变，Prometheus 抓取消耗可忽略。
- **代码成本**：自研扩展约 80–120 行 + 渲染测试；不引入新依赖。
- **替代方案——引入 `prometheus` crate**：获得现成 histogram/registry，但引入约 15+ 传递依赖（含 parking_lot、thiserror、protobuf 可选等），与"零依赖渲染器 + 固定基数"的现有设计哲学冲突；且现有调用点是静态 AtomicU64 直写模式，改造面大于自研扩展。**不推荐**。

## 结论

**结论：扩展自研渲染器支持 histogram（方向二），而非引入 prometheus crate。** 实施建议作为独立 issue 排期：先加 `Histogram` 类型与渲染，再在 gateway frontdoor 与 LLM 上游两处接入 TTFT/端到端延迟。histogram 落地同时缓解 issue 中"计费零指标"的供给侧问题（结算耗时、fail-open 速率可观测）。
