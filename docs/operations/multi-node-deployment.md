# 三节点部署基线

本目录提供一个可复核的三节点网关基线：两个 `frontdoor` 节点承接入口流量，
一个 `background` 节点运行后台任务。PostgreSQL、Redis 和 tunnel relay 均为外部
服务；compose 不会在每台机器上悄悄启动独立数据库。

## 启动

1. 复制 `deploy/multi-node/.env.node-*`，替换镜像 digest、数据库/Redis 凭据和
   relay 域名。不要把真实凭据提交到仓库。
2. 在每台主机只保留对应节点的 env 文件，然后运行：

   ```sh
   tools/operations/check_multi_node_preflight.sh .env.node-frontdoor-1
   docker compose -f deploy/multi-node/docker-compose.yml up -d frontdoor-1
   ```

   三个服务也可由同一编排器启动：`docker compose -f deploy/multi-node/docker-compose.yml up -d`。
3. 入口负载均衡器只把 HTTPS 流量转发到 `frontdoor-1/2:8084`，健康检查 `/health`，
   并将其固定在 `AETHER_GATEWAY_TRUSTED_INGRESS_CIDRS`。该 CIDR 必须只包含实际
   反向代理网段；不要把公网网段加入信任列表。background 不应暴露公网端口。

`AETHER_GATEWAY_INSTANCE_ID` 必须在节点间唯一。需要跨节点 tunnel owner 转发时，
每个节点必须提供能从其他节点访问的 `AETHER_TUNNEL_RELAY_BASE_URL`，relay 本身
应验证签名并限制来源网络。

## PostgreSQL 连接池预算

示例按 PostgreSQL `max_connections >= 160` 预留：frontdoor 两节点各 40，background
60，迁移/管理员和监控预留 20。实际部署按公式重新计算：

`sum(node postgres_max_connections) + 20 <= postgres max_connections`

若增加副本，先下调每实例上限或提高数据库上限，再发布；不要依赖每核自动值。Redis
同样必须是共享实例，且所有节点使用相同 DB/key prefix。Redis 默认非持久化，丢失后
限额计数和 continuation history 按 [Redis runbook](redis-runtime-runbook.md) 的
fail-open/recovery 语义处理，不得当作财务账本。

## 故障演练（隔离环境）

- 停止 `frontdoor-1`，确认负载均衡器摘除该节点且请求仍由 `frontdoor-2` 返回 200，
  再恢复并观察 `/health`。
- 暂停 Redis 连接，确认告警出现、限流/运行时状态按 runbook 的降级语义工作；恢复
  后检查 runtime recovery、usage queue 和 continuation history 错误指标。
- 暂停 relay-1，向原 owner 发起 tunnel 请求，确认请求失败可观测且不会转发到未信任
  的地址；恢复 relay 后重试。

演练必须记录时间、节点、镜像 digest、请求量、p95/p99、DB pool 使用率、Redis
错误、usage queue pending/lag/DLQ 与 outbox pending。仓库无法在 CI 中证明真实三
节点、真实负载均衡或故障恢复；`verify_multi_node_assets.sh` 只验证 env 契约和
compose 解析，不能替代上述隔离环境证据。

