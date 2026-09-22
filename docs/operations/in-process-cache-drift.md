# 进程内缓存漂移（多节点语义）

状态：文档化评审结论（2026-09-22，#224 批次）。权威行为合同见 ADR
[gateway-cache-consistency](../adr/gateway-cache-consistency.md)；本文件面向
部署与运营，回答“多副本下哪些状态会漂移、窗口多大、能调什么”。

## 结论先行

Gateway 约 16 处进程内 TTL 缓存（`ExpiringMap`）。**写入路径只失效当前实例
的缓存，其他实例依赖 TTL 到期再读权威源**；没有跨节点广播/失效通道（无
pub/sub）。需要“禁用即全局生效”语义的操作，当前可承诺的最坏窗口如下表。

## 漂移面与窗口

| 缓存 | 跨实例可见性窗口（默认） | 可调项 | 多副本影响 |
| --- | --- | --- | --- |
| 认证上下文 / API key 快照 | 正向 60s；命中后强读复核约 10s（复核限 ≤10s）；本地与数据层快照各 30s | `AETHER_GATEWAY_AUTH_CONTEXT_CACHE_REFRESH_INTERVAL_SECS`（复核间隔，1–10s）、`AETHER_GATEWAY_AUTH_CONTEXT_NEGATIVE_CACHE_TTL_SECS`（负向 TTL） | A 节点禁用 key 后，B 节点普通命中路径最长约 30s 放行；带复核路径约 10s |
| 系统配置 | 新鲜期 30s；后台刷新失败时最长返回 5 分钟旧值 | 授权敏感调用走 strong 读 | 配置变更（如开关类）在其他节点最长 5 分钟才不新鲜 |
| Provider catalog | 数据层读缓存 5s（不是所有路由结果的统一上界） | — | catalog 变更数秒内收敛；路由相关本地缓存各有 TTL |
| IP 黑白名单 | 本地读缓存 1s（可至 30s） | `AETHER_GATEWAY_SECURITY_CACHE_TTL_MS`（≤30000） | 封禁 IP 在其他节点最长 TTL 内仍可访问；保持默认 1s |
| 用户设置 / 用户群组 | 各 30s | — | 设置变更最长 30s 漂移 |
| candidate page / 调度亲和 / rate limit 计数视图 | 各自 TTL（candidate page 见 `AETHER_GATEWAY_CANDIDATE_PAGE_CACHE_TTL_MS` / `_STALE_TTL_MS`） | 对应 `AETHER_GATEWAY_*_CACHE_TTL_MS` | 调度与准入视图秒级漂移，不影响安全边界 |

不受影响的面：跨节点请求/WS 分布式闸（Redis 信号量）、RPM 与日用量限额
（Redis；multi-node 拓扑自动禁用 local fallback，避免限额 ×N）、31 类单例
后台任务（Redis 租约 + fencing）、usage 队列（Redis Stream 消费组，可多实例）、
tunnel attachment 路由（Redis + owner relay）。

## 运营指引

1. 撤销 API key / 封禁 IP 等安全操作后，按上表最坏窗口评估残留风险；高风险
   场景先用 LB 摘节点（`/readyz` 失败即摘流）再操作。
2. 不要把 `AETHER_GATEWAY_SECURITY_CACHE_TTL_MS` 调大；默认 1s 是安全敏感
   路径的最小漂移预算。
3. Redis 故障期注意：一致性优先开关（`AETHER_CONSISTENCY_FIRST=true`）使
   限额检查失败即拒绝，避免 fail-open + 进程内缓存叠加放大放行面。
   语义见 [redis-consistency-first ADR](../adr/redis-consistency-first.md)。

## 后续项（不在 #224 本批）

- 鉴权类缓存（认证上下文 / API key 快照 / 用户群组）的跨节点失效通道：
  Redis pub/sub 或版本号检查机制，及配套的故障语义（broker 不可用时降级为
  TTL 窗口）。实现前需先在 ADR 定义契约。
- `DistributedConcurrencyGate` 悬空 API 的删除/改名（issue #224 建议 4，
  代码清理项）。
