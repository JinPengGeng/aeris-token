# 架构决策与请求数据流

本页是架构目录的入口，记录当前可核对的 ADR 状态和 Gateway 请求的主要边界。
ADR 的状态只描述已经落地的范围；延期的接线、部署或生产演练不会因为文档存在而被视为完成。

## ADR 索引

| 文档 | 状态 | 已落地范围 | 仍延期的范围 |
| --- | --- | --- | --- |
| [ADR-0044：Emergency chain domain boundary](adr-0044-emergency-chain-domain.md) | Accepted（Issue #44 administrator operations v1 已实现） | Gateway 管理员固定顺序 model-test 路由、五分钟 owner-bound grant、PostgreSQL issue/read/revoke/consume、事务耦合审计和逐目标强读 | 公共/租户调度的 opaque permit、权威 attempt ledger 与 CAS send boundary |
| [ADR-0045：Signed provenance for tunnel release upgrades](adr-0045-signed-tunnel-release-provenance.md) | Accepted（核心验签已实现） | 发布清单签名与离线验证、手工/heartbeat 升级门禁 | 完整发布矩阵和生产轮换演练 |
| [ADR-0046：Bounded gateway readiness and health contract](adr-0046-readiness-health-contract.md) | Accepted | `/health`、`/ready` 的边界、依赖探测与超时合同 | 生产部署容量与告警验收 |
| [ADR-0050：Tunnel signing key rotation overlap](adr-0050-tunnel-signing-key-rotation.md) | Accepted（部分实现） | key ID、有效期、重叠窗口和撤销语义 | 持久化与 wire integration |
| [ADR-0051：Provider and data-layer extension surfaces](adr-0051-extension-surfaces.md) | Accepted（Issue #222 bounded contract） | PostgreSQL-only data boundary、provider registration points、measured change baseline | Cross-capability provider registry、trait splitting、migration convergence |
| [ADR-0052：Cross-node invalidation channel for auth/config in-process caches](adr-0052-cache-invalidation-channel.md) | Proposed（Issue #509，#224 遗留项） | Redis pub/sub 失效广播契约（channel、消息格式、发布/订阅点、失败语义） | 全部实现、多节点一致性测试、第一期范围外缓存域 |

编号沿用仓库已有记录，不补造缺失的历史 ADR-0001 至 ADR-0043。新增记录应使用下一个
可追溯编号，并在标题、状态、相关 Issue 和延期范围中说明来源；描述性 ADR（见
[`docs/adr/README.md`](../adr/README.md)）与本目录的编号记录可以并存，但不得重复声称同一
行为已经接线。

## 请求数据流

下图按 Gateway 的责任边界概括一次公开请求。具体协议适配器和 provider 实现可能不同，
但每一步的鉴权、限流、计费和失败语义都应能在对应源码、测试或运行手册中定位。

```text
HTTP/WS ingress
  -> request size / timeout / CORS guard
  -> authentication and principal resolution
  -> route + model/provider candidate selection
  -> upstream transport and stream observation
  -> usage event enqueue (Redis stream)
  -> usage worker persistence (PostgreSQL)
  -> response, audit record and operational metrics
```

关键边界：

- ingress 的 liveness/readiness 不等于业务依赖可用；健康合同见 [ADR-0046](adr-0046-readiness-health-contract.md)。
- 路由只选择候选和传输策略；余额、配额及最终 usage 结算由数据/usage 层负责，不能用路由成功替代账务成功。
- 经隧道转发的出口流量遵循 [隧道信任模型](tunnel-trust-model.md)：Gateway 管理面与隧道 agent 之间是单向绝对信任，升级链路由编译进二进制的 ed25519 信任集合锚定。
- Redis stream 是 usage 事件的传递层，不是完整账本；重试、DLQ 和保留策略见 [`docs/adr/usage-runtime-retry-dlq.md`](../adr/usage-runtime-retry-dlq.md)。
- PostgreSQL 写入失败、部分提交和恢复边界应按 [备份恢复演练](../operations/backup-restore-drill.md) 的证据要求验收；本页不宣称生产灾备已完成。
- Emergency chain 与普通候选路由隔离。ADR-0044 的管理员 v1 通过独立
  `admin:provider_query` 路由执行，不改变默认请求路径；公共/租户调度的 permit/ledger/CAS
  设计仍未接线。

## 维护要求

修改公共请求边界、数据语义或部署合同时，在同一 PR 更新相关 ADR 与本索引，并链接至少一个
调用入口和一个可复核测试。发生行为改变时，将旧记录标为 Superseded 并保留替代链接。
