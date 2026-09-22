# Gateway 压测基线（S1–S5 / TPS）

本目录是网关容量验收的唯一入口：驱动脚本、`gateway_pressure_probe` 负载发生器、
分档 checker 与 fixtures seed。S1–TPS 的硬阈值契约（1000 rps 下限、DB 池压力、
锁等待=0、Redis lane 延迟、usage 队列排空、DLQ=0 等 40+ 项）在
`check_gateway_stage_report.js` 中定义。

## 分档

| 档 | 规模 | 驱动 |
| --- | --- | --- |
| S1 | 1000 请求 / 1000 并发 | `run_gateway_mock_streaming_stage.sh` |
| S2–S5 | 3k–10k（S5 为 soak） | 同上，`PRESSURE_STAGE=S2..S5` |
| TPS | 30k 请求 / 600 并发 / 1000 rps 下限 | `run_gateway_20k_h2_low_noise.sh` |
| 真实画像 | realistic-stream | `run_gateway_realistic_profile.sh` |

## 环境要求

真实 Postgres + Redis + 网关 + mock 上游（`mock_openai_upstream`，默认
`127.0.0.1:18181`）。先起网关（`database_mode=auto` 会建表），再 seed：

```sh
cargo run --locked -p aether-integration-tests --bin gateway_pressure_seed
source /tmp/aether_local_env.sh
cargo run --locked -p aether-integration-tests --bin mock_openai_upstream -- --chunks 8 --chunk-delay-ms 20 &
PRESSURE_STAGE=S1 ./run_gateway_mock_streaming_stage.sh
node check_gateway_stage_report.js --stage S1 /tmp/aether_gateway_pressure_s1_1k.json
```

升档前必须确认上一档排空；结果（请求量、并发、p95/p99、报告路径、环境规模、
节点数、镜像 digest）应附在对应 Issue/PR，不能只记录“压测通过”。

## CI 入口

`.github/workflows/pressure-baseline.yml`（每周日 03:23 UTC + 手动触发）在
ubuntu runner 上以 60 请求 / 10 并发跑 S1 dry-run，验证本工具链（seed、mock
upstream、probe、checker、报告产出）端到端可用并上传报告 artifact。它**不是**
容量门槛：S1 硬阈值按 1000 并发标定，在 CI 中仅作 advisory 运行
（`continue-on-error`）。容量回归必须在专用压测硬件上按上表分档执行。

## 文档

- 多节点部署与 DB 池预算拆分：`../../docs/operations/multi-node-deployment.md`
- Redis 故障降级语义与一致性优先开关：`../../docs/adr/redis-consistency-first.md`
- 多节点验收流程决策：`../../docs/issue-triage/issue-224-multi-node-decision.md`
- 容量曲线回归（capacity_curve_baseline）跑法与基线记录位置：`../../docs/operations/capacity-curve-baseline.md`
