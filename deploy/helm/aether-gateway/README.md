# aether-gateway Helm chart

最小可用 chart：frontdoor Deployment（多副本，承接入口流量）+ background
Deployment（单副本后台任务）+ ClusterIP Service + 共享 Secret。数据库与 Redis
为外部托管服务（与 compose 生产基线同构），chart 不内置 StatefulSet。

## 为什么不内置 Postgres/Redis

多节点拓扑的权威数据与协调层必须共享；K8s 内自研 Postgres/Redis 高可用
operator 超出本评审批的可控工作量，且有成熟外部方案。需要 in-cluster 数据
层时，先接入云厂商托管实例或已评审的 operator，再回填 values。

## 用法

```sh
helm install aether ./deploy/helm/aether-gateway \
  --set image.digest=sha256:... \
  --set secrets.jwtSecretKey=... --set secrets.encryptionKey=... \
  --set database.url=postgresql://... --set redis.url=redis://:...@...:6379/0 \
  --set gateway.trustedIngressCidrs=10.20.0.0/16
```

## 多副本纪律

- 扩 frontdoor 副本前先按
  `docs/operations/multi-node-deployment.md` 的公式重算
  `sum(node max_connections) + 20 <= postgres max_connections`。
- `INSTANCE_ID` 由 pod 名自动注入，天然唯一。
- 进程内缓存漂移窗口与可调 TTL 见
  `docs/operations/in-process-cache-drift.md`；chart 默认
  `gateway.consistencyFirst=true`（对齐 standalone compose 的多节点默认）。
- background 默认单副本；扩 >1 前先确认目标 TASK_KEY 是否允许多实例
  （usage queue worker 可多实例，其余单例任务靠 Redis 租约选主，多副本会
  空转）。

## 已知取舍

- Secret 走 chart 内联 values（模板渲染），生产应替换为 externalSecrets /
  existingSecret，本 chart 未集成。
- 未提供 Ingress 模板：入口 LB/Ingress 由各环境自己的网关层承担，Service
  暴露 ClusterIP 即可。
- 无 HPA/VPA/PDB：容量伸缩以压测基线（`tools/pressure/`）为准入门槛，
  自动伸缩策略待有生产指标后再定。
