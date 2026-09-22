# Aether 多节点部署资产

本目录提供多节点（multi-node）拓扑的两类 compose 资产，均与单节点资产
（根目录 `docker-compose.single-node.yml`）保持一致的 env 命名与安全基线
（非 root、只读根文件系统、cap_drop ALL、no-new-privileges、日志轮转）。

| 文件 | 用途 |
| --- | --- |
| `docker-compose.yml` | 生产基线：两 frontdoor + 一 background，PostgreSQL / Redis / tunnel relay 为外部托管服务，不在编排内启动数据库 |
| `docker-compose.standalone.yml` | 隔离环境一体化拓扑：内置 Postgres、AOF 持久化 Redis、两 frontdoor + 一 background、nginx L7 负载均衡，用于功能验证与压测前置演练 |
| `.env.node-*` | 生产基线各节点 env 模板（复制后替换 REPLACE_* 占位） |
| `.env.standalone.example` | standalone 编排的单份 env 模板 |
| `nginx/lb.conf` | standalone 编排的 nginx 反代配置（least_conn、/readyz 就绪检查、frontdoor 间 keepalive） |

## 快速开始（standalone）

```sh
cp deploy/multi-node/.env.standalone.example deploy/multi-node/.env.standalone
# 编辑替换所有 REPLACE_* 占位
docker compose --env-file deploy/multi-node/.env.standalone \
  -f deploy/multi-node/docker-compose.standalone.yml up -d
# 入口在 http://127.0.0.1:${LB_PORT:-8080}，仅经 LB 暴露两个 frontdoor；
# background 不暴露任何端口。
```

生产基线（外部 Postgres/Redis）的启动步骤、DB 连接池预算拆分公式、故障演练
与容量验收流程见 [docs/operations/multi-node-deployment.md](../../docs/operations/multi-node-deployment.md)。

## 多副本注意事项（进程内缓存漂移）

两个 frontdoor 副本各自维护进程内 TTL 缓存，**写操作只失效本实例的缓存，
其余实例依赖 TTL 到期**。典型窗口：认证上下文正向 60s / 命中后强读复核约 10s
（`AETHER_GATEWAY_AUTH_CONTEXT_CACHE_REFRESH_INTERVAL_SECS`，上限 10s）、
本地 API key 快照 30s、系统配置新鲜期 30s（失败时最长 5 分钟旧值）、
IP 黑白名单本地读缓存 1–30s
（`AETHER_GATEWAY_SECURITY_CACHE_TTL_MS`）。即在 A 节点禁用 key 后，B 节点
最长约 30s（命中复核路径约 10s）仍可能放行。

跨节点协调走共享 Redis（分布式锁/信号量/usage Stream/租约），不受进程内缓存
影响；RPM 限速 local fallback 在 multi-node 拓扑下已自动禁用，日用量限额配合
`AETHER_CONSISTENCY_FIRST=true` 可避免 Redis 故障期的限额 ×N 与 fail-open 放大。

完整漂移面、可调 TTL 与“跨节点失效通道”后续项见
[docs/operations/in-process-cache-drift.md](../../docs/operations/in-process-cache-drift.md)
与 ADR [gateway-cache-consistency](../../docs/adr/gateway-cache-consistency.md)。

## 静态自检

```sh
tools/operations/verify_multi_node_assets.sh
```

校验全部 env 模板契约、两份 compose 的 YAML 解析与（docker 可用时）compose
config。它不替代隔离环境的多节点/故障演练证据。
