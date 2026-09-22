# 压测容量基线（aether-testing）

本文把 `crates/aether-testing` 下容量基线工具的部落知识落成文档：标准跑法、
环境前提、结果记录位置。S1–S5/TPS 分档验收契约的入口是
[tools/pressure/README.md](../../tools/pressure/README.md)，本文不重复其阈值
定义。

## 工具分层

| 工具 | 性质 | 用途 |
| --- | --- | --- |
| `capacity_curve_baseline`（integration bin） | 进程内 harness，自带 mock 上游 | 容量曲线：并发档位 8→256，找饱和点（p95 超过延迟预算 ×4 判定） |
| `gateway_pressure_probe` / `http_load_probe` / `runtime_redis_pressure` / `redis_worker_baseline`（loadtools bins） | 真实网关压测驱动 | 配合 `tools/pressure/` 脚本打真实部署 |
| `tools/pressure/check_gateway_stage_report.js` | 40+ 硬阈值 checker | S1–S5/TPS 报告验收 |

## capacity_curve_baseline 标准跑法

```sh
cargo run --locked -p aether-integration-tests --bin capacity_curve_baseline -- \
  --output /tmp/capacity_curve_baseline.json
```

默认参数：点 `8,16,32,64,128,256`、每点请求 = 档位 × 8、sync 延迟 75ms、
stream 分块 25ms、tunnel hold 75ms、超时 10s、饱和判据 p95 > 预算 ×4。覆盖
五个场景：gateway sync / gateway stream / execution-runtime sync /
execution-runtime stream / gateway tunnel stream。输出 JSON（stdout + 可选
`--output`），含每点并发、吞吐、reject/error 计数、p50/p95/p99 与
in-flight/available permits 快照。

环境前提：本机可编译 workspace（`--locked`）；harness 自起网关/运行时与 mock
上游，**不需要**外部 Postgres/Redis。`init_test_runtime_for` 提供隔离运行时，
测试之间不共享状态。

## 真实部署压测（S1→TPS 链）

前提：真实 Postgres + Redis + 网关（多节点用
`deploy/multi-node/docker-compose.standalone.yml` 起一体化拓扑，LB 端口作
入口），mock 上游 `127.0.0.1:18181`。标准顺序：

```sh
cargo run --locked -p aether-integration-tests --bin gateway_pressure_seed
cargo run --locked -p aether-integration-tests --bin mock_openai_upstream -- --chunks 8 --chunk-delay-ms 20 &
PRESSURE_STAGE=S1 ./tools/pressure/run_gateway_mock_streaming_stage.sh
node tools/pressure/check_gateway_stage_report.js --stage S1 /tmp/aether_gateway_pressure_s1_1k.json
```

升档前必须确认上一档排空（usage 队列、DLQ=0）。

## 基线数值记录位置

1. **CI dry-run**：`.github/workflows/pressure-baseline.yml`（周日 03:23 UTC
   定时 + 手动）在 ubuntu runner 上 60 请求/10 并发跑 S1 工具链，报告 JSON
   作为 workflow artifact 保存（`aether-pressure-baseline-*`）。它是工具链
   健康检查，不是容量门槛。
2. **容量门槛**：S1（1000 并发）与 TPS（1000 rps 下限）必须在隔离压测硬件
   执行，checker 报告 + 环境规模（节点数、镜像 digest、DB/Redis 规格、
   p95/p99）附到对应 Issue/PR 才算落地。
3. **容量曲线回归**：`capacity_curve_baseline --output` 的 JSON 建议存入
   `artifacts/capacity-curve/<日期>-<commit>.json`（目录已存在则复用；
   不入 git，随 issue/PR 附件引用）。

## 与多节点发布的接入

多节点发布前的验收顺序：单节点容量曲线回归（确认无退化）→ standalone
三节点拓扑 S1 →（重大变更）TPS 档。任何一档未过硬阈值即阻断发布，报告
附 PR。
