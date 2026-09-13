# Issue 218：Redis 生产持久化覆盖决策

日期：2026-09-13

## 决策

默认 `docker-compose.yml`、`docker-compose.single-node.yml` 和本地覆盖继续使用
临时 Redis（`/tmp`、禁用 AOF/RDB），以保持 `make dev` 和测试的快速、可丢弃语义。
需要跨重启保留 usage stream、DLQ 或运行时协调状态的部署，必须显式叠加
`docker-compose.redis-production.yml`：它使用命名卷 `/data`、AOF
`appendfsync everysec`，并启用受控的 RDB 快照间隔。

```sh
docker compose -f docker-compose.yml \
  -f docker-compose.redis-production.yml up -d
```

`REDIS_PASSWORD` 仍由 Compose 的 `:?` 守卫强制提供；空值会使配置解析失败，不会
启动一个无认证的 Redis。生产环境必须先验证卷备份、恢复演练和磁盘告警，再采用该覆盖。

## TLS 边界

该覆盖只解决本地 Redis 的持久化，不伪造 Redis TLS。跨主机或不受信任网络的生产部署
应使用支持 TLS 的托管 Redis，并将应用的 `REDIS_URL`（或
`AETHER_RUNTIME_REDIS_URL`）设置为 `rediss://...`；当前内置 Redis 服务仍绑定回环端口，
不能作为跨网络 TLS 终结点。若运行时客户端/依赖未启用 `rediss`，部署必须在启动前失败，
不得通过关闭证书校验降级为明文连接。

## 验证

`tests/redis_production_profile_test.py` 断言基础配置仍为临时 Redis，生产覆盖必须包含
命名卷、AOF/RDB 参数，并验证缺失 `REDIS_PASSWORD` 时 `docker compose config` 非零退出。
该测试不连接真实 Redis，也不声称完成备份恢复或 TLS 验证；这些属于部署验收工作。
