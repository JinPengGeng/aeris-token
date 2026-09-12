# #217 可观测性复核与交付拆分

日期：2026-09-12  
复核基线：fork `JinPengGeng/aeris-token` 的 `origin/main`，提交 `1ad07d26e3d7a1e3eba653289cebf941da252c8c`

## 结论

Issue #217 的总体风险判断仍为 P1，但原报告是跨边界跟踪项，不能作为一个 PR 一次性实现。本次复核确认计费终态失败仍只有结构化 `warn` 事件，`/readyz` 仍不检查依赖，仓库仍没有可直接部署的 Prometheus/Alertmanager 规则。另一方面，报告中的两项事实已经被后续代码部分修正：网关已经暴露多类运行时/并发/数据库/Redis/usage 指标；三个 compose 文件已经为 PostgreSQL/Redis 添加 healthcheck（应用容器仍没有 healthcheck）。

## 当前事实矩阵

| 主题 | 当前证据 | 判断 | 优先级 |
| --- | --- | --- | --- |
| 计费 enrichment / settlement | `crates/aether-usage/runtime/src/worker.rs:820-829`、`runtime.rs:5311-5321`、`apps/aether-gateway/src/async_task/runtime.rs:416-445` 只记录失败事件；`aether-billing` 和 `aether-wallet` 没有自己的 metric exporter | 确认缺口，直接影响资损发现 | P1 |
| 请求 RED | `apps/aether-gateway/src/state/core.rs:1676-1877` 已有服务、并发、DB/Redis、usage、阶段等快照；未形成稳定的 route/status/provider 请求计数契约 | 原“零请求指标”表述过宽；仍需独立 RED 设计 | P1 |
| 告警资产 | `.github/workflows/scripts/tests/test-sync-minimal-alerts.sh` 仅测试同步脚本；没有 Prometheus rule、Alertmanager 配置或 Grafana dashboard 交付物 | 确认缺口 | P1 |
| `readyz` | `apps/aether-gateway/src/api/core.rs:90-99` 忽略 `AppState`，固定返回 `status=ready`，并将 `gate_readiness=false` 写死 | 确认缺口；涉及摘流语义 | P1 |
| compose healthcheck | `docker-compose.yml`、`docker-compose.single-node.yml`、`docker-compose.release-local.yml` 已为 postgres/redis 配置 healthcheck；`app` 没有 | 原报告部分过时；应用 readiness 仍需单独处理 | P2 |
| 日志格式 | compose 和 gateway CLI 默认 `AETHER_LOG_FORMAT=pretty` | 确认，但属于兼容性/部署策略变更 | P2 |

## 决策：拆成独立交付

1. **217-A 计费失败计数（P1，先做）**：在现有 gateway/usage runtime 共享的低基数 metrics owner 上增加 `*_billing_enrichment_failures_total`、`*_settlement_failures_total` 和 fail-open 计数；标签只允许稳定的 `component`/`operation`，禁止 request ID、用户、模型和错误文本。每个事件点必须有单元测试，`/metrics` 必须渲染 counter。
2. **217-B 最小告警规则与 runbook（P1，依赖 A）**：交付 Prometheus rule 示例和运维说明，至少覆盖计费失败、usage DLQ、fail-open、provider 5xx。规则需写明阈值、持续时间、严重级别、通知去重和回滚；不能声称仓库内置 Alertmanager。
3. **217-C 请求 RED（P1，独立设计）**：在稳定 `route_class`、`status_class`、`provider` 维度记录请求总数/错误数/延迟；先做基数预算与兼容矩阵，再实现 counter，histogram 另行评估。
4. **217-D 真实 readiness（P1，独立评审）**：定义 DB、Redis、关键后台 worker 的检查、超时和降级策略；区分 liveness 与 readiness，测试依赖失败时的 HTTP 状态和响应契约。不能直接把 `/health` 改成阻塞式探测。
5. **217-E 默认 JSON 日志（P2）**：先在部署文档与采集器兼容性验证后再改默认值；本地开发默认 pretty 的行为应保留可选开关。

## 暂不在本次复核中实现的内容

不在没有统一 metrics owner、生命周期和告警阈值的情况下直接改计费 crate 或 readiness。计费失败事件跨 usage worker、terminal runtime 和视频任务 finalizer，贸然各自增加全局状态会产生重复计数或无法从 gateway `/metrics` 汇总。真实 readiness 还需要确认数据库/Redis 客户端是否允许有界探测，属于运行时行为变更，应单独 PR、回归测试和部署演练。

## 验收与协作门禁

每个拆分项都必须：在本 fork 的 Issue/决策记录中固定契约；使用独立分支和 PR；包含 focused tests 与 `git diff --check`；等待 required CI 全绿后 squash merge；随后更新 #217 的交叉引用、Project 状态和交付 TODO。任何阶段不得修改 upstream。真实数据库/Redis 演练不以本地单元测试替代。

