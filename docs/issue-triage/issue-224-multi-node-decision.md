# Issue #224：多节点与水平扩展决策记录

日期：2026-09-12  
范围：`JinPengGeng/aeris-token` fork；不修改 upstream。

## 结论

Issue #224 的代码判断基本准确，收益为高，原因是它影响跨节点限额、后台任务单例、隧道 owner 路由和发布验收。当前启动校验已经阻止最危险的静默降级，但仓库仍缺少可复制的多节点启动资产和容量验收入口，因此 Issue 保持 **open / P1**，不能因已有校验而关闭。

已核验的事实：

- `apps/aether-gateway/src/main.rs::validate_deployment_topology` 在 multi-node 下拒绝 `node_role=all`、缺 SQL/Redis、memory runtime backend 和本地视频任务存储；`INSTANCE_ID`、relay URL 缺失只告警。
- `docker-compose.yml`、`docker-compose.single-node.yml` 均为单 app 实例，没有多节点模板、节点级 env 示例或 DB 连接池预算说明。
- `tools/pressure/check_gateway_stage_report.js` 提供 S1-S5/TPS 阈值检查，且已有 `run_gateway_*` 驱动脚本；此前没有面向运维的文档入口或 preflight 检查。
- Redis 故障时限流/部分运行时状态存在 fail-open 或本地降级语义；进程内 TTL 缓存也不会跨节点即时失效。这些是明确的运行边界，不应在部署文档中宣称强一致。

## 本轮交付

新增 `tools/operations/check_multi_node_preflight.sh`。它不执行 env 文件，只读取键值并检查：

1. topology 为 `multi-node`，且 role 不是 `all`；
2. 配置共享 SQL 与 Redis URL；
3. runtime backend 不是 `memory`；
4. 未设置 `AETHER_GATEWAY_VIDEO_TASK_STORE_PATH`；
5. 对缺少 instance ID / relay URL 发出可见告警，但按现有代码语义不阻断 frontdoor 节点。

示例：

```bash
tools/operations/check_multi_node_preflight.sh .env.node-frontdoor
```

脚本只打印 presence 和拓扑信息，不打印 URL、密码或 token。

## 容量验收（发布前手工门）

先对每个节点执行 preflight，再在隔离环境运行真实 AI 调度/结算链路的分档压测。健康检查吞吐不能替代业务压测。

```bash
PRESSURE_STAGE=S1 ./tools/pressure/run_gateway_mock_streaming_stage.sh
node tools/pressure/check_gateway_stage_report.js --stage S1 /tmp/aether_gateway_pressure_s1_1k.json
```

升档到 S2-S5 与 `tps` 前，必须确认上一档排空；以 checker 的硬阈值为准，至少保留请求量、并发、p95/p99、DB pool pressure、Redis runtime 错误、usage queue pending/lag/DLQ、outbox pending 和后台任务异常退出指标。结果、环境规模、节点数、镜像 digest 与报告路径应附在对应 Issue/PR，不能只记录“压测通过”。

## 后续工作与关闭条件

- P1：提供官方 multi-node compose/部署文档，包含三节点 env 清单、入口负载均衡、`TRUSTED_INGRESS_CIDRS`、relay URL 和 DB pool 拆分公式。
- P1：将 S1-S5/TPS 验收纳入可手工触发的 CI/定时环境，并保存报告 artifact（不在普通 PR CI 中启动真实基础设施）。
- P1：为 Redis 故障、缓存失效窗口和限流 ×N 降级语义写运行手册，并决定一致性优先开关。
- P2：移除或重命名未被生产路径调用的 `DistributedConcurrencyGate`，避免误用。

只有上述部署路径、容量 artifact 和故障语义文档均可复核后，才建议关闭 Issue #224。
