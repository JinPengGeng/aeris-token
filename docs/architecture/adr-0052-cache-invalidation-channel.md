# ADR-0052: Cross-node invalidation channel for auth/config in-process caches

## Status

Proposed for Issue #509（#224 收敛批遗留项，2026-09-22）。本记录只定义契约与
取舍，不声称任何广播/失效通道已经接线；实现与多节点一致性测试按本 ADR 验收。

## 背景

Gateway 约有 16 处进程内 TTL 缓存（`ExpiringMap`）。当前写路径只失效**当前
实例**的缓存，其余实例依赖 TTL 到期再读权威源，没有跨节点失效通道（无
pub/sub）。漂移面与窗口在
[in-process-cache-drift](../operations/in-process-cache-drift.md)（PR #508 交付）
中逐表列出，权威行为合同见
[gateway-cache-consistency](../adr/gateway-cache-consistency.md)：

- 认证上下文 / API key 快照：普通命中路径最长约 30s，带强读复核路径约 10s；
- 系统配置：新鲜期 30s，后台刷新失败时最长 5 分钟旧值；
- IP 黑白名单本地读缓存：1s（可配置至 30s）；
- 用户设置 / 用户群组：各 30s；
- candidate page / 调度亲和 / rate limit 计数视图：各自 TTL，只影响调度与
  准入视图，不影响安全边界。

在单节点部署下这些窗口只是“本实例写后多久生效”的实现细节；PR #508 交付的
multi-node compose / Helm chart 与
[multi-node-deployment](../operations/multi-node-deployment.md) 使多节点成为有
资产的正式拓扑，漂移从理论问题变成现实问题：A 节点禁用 API key 后，B 节点
最长约 30s 仍放行。撤销与封禁类安全操作的可承诺窗口需要收敛。

## 候选方案

### 方案 A：Redis pub/sub 失效广播

管理员写路径在提交变更后向 Redis channel 发布一条轻量失效消息；每个节点
启动时订阅该 channel，收到消息后按消息中的缓存域与键失效本地 `ExpiringMap`
条目。

- 有利：仓内 Redis 已是硬依赖（多节点拓扑启动期强制校验），且已有
  连接管理设施——`aether-runtime/state` 的 Redis 后端基于
  `redis::aio::ConnectionManager` 多路复用连接（client.rs），usage Stream
  消费组、分布式锁、租约均复用它；失效消息是无状态易失事件，pub/sub
  “尽力投递”语义恰好匹配。
- 不利：允许丢消息——订阅方短暂断连期间的消息永久丢失；需要处理断连
  重订与启动竞态（先订阅再服务流量，或接受启动瞬间窗口）。

### 方案 B：版本戳 + 短 TTL 校验

给每个缓存项附带全局版本戳（Redis 计数器 / 时间戳），读路径命中本地缓存前
先比对版本，不一致即回源。

- 有利：无新组件，理论上每次读都能发现漂移，窗口可收敛到秒级以下。
- 不利：给每条读路径（含热路径认证）增加一次 Redis 往返或本地批量校验
  的侵入；为覆盖 16 处缓存需要逐处定义版本粒度与回源逻辑；Redis 故障时
  校验本身需要降级语义，复杂度转移到读路径。侵入面明显大于方案 A。

## 决策

采用**方案 A：Redis pub/sub 失效广播**，第一期只接鉴权/配置类高敏缓存。

### Channel 与消息格式

- Channel：`aether:cache-invalidate`（单 channel，消息内区分缓存域）。
- 消息为 JSON，字段：
  - `v`：消息格式版本（从 1 开始；订阅方遇到不认识的大版本号忽略并记
    指标，保证可演进）；
  - `domain`：缓存域，取值 `auth_context` | `system_config` |
    `security_ip_list` | `user_provisioning`（第一期清单）；
  - `key`：域内键（如 API key id、配置项 key、IP/CIDR、user id）；
    全量失效用通配值 `*`；
  - `origin`：发布实例的 `AETHER_GATEWAY_INSTANCE_ID`，订阅方收到自己
    发布的消息时跳过本地重复失效（本实例写路径已同步失效）；
  - `ts`：发布时刻（epoch 秒，用于日志与滞后排障，不参与正确性）。
- 消息只携带失效指令，不携带数据；正确性永远由“本地失效 → 下次读回源
  权威存储”保证，与现有写路径失效语义一致。

### 发布点（管理员写路径，提交后发布，失败只记日志不阻断写）

- API key / 用户 / 钱包等鉴权写路径（现有
  `invalidate_auth_context_cache` 调用点之后追加广播，`domain=auth_context`）；
- 系统配置写路径（`domain=system_config`，按 key 或 `*`）；
- IP 黑白名单写路径（`domain=security_ip_list`）；
- 用户设置 / 群组写路径（`domain=user_provisioning`）。

### 订阅点

- 各节点启动时经既有 Redis 连接管理建立 pub/sub 订阅（独立连接，
  与 `ConnectionManager` 请求/响应通道分离），就绪前完成首次订阅，避免
  启动竞态窗口；
- 收到消息后按 `domain` + `key` 调用对应的本地失效入口（即现有写路径
  使用的同一组失效函数），不新增第二套失效逻辑；
- 订阅任务纳入现有后台 worker 监督体系，断连按指数退避重订。

### 范围分期

- 第一期（本 ADR 范围）：上列鉴权/配置类高敏缓存——认证上下文 / API key
  快照、系统配置、IP 黑白名单、用户设置 / 用户群组。
- 其余缓存面（provider catalog、candidate page、调度亲和、rate limit
  计数视图等）保持 TTL-only：它们的漂移只影响调度与准入视图，不影响安全
  边界，接入广播的收益不抵复杂度。

## 失败语义（fail-open 有界）

- pub/sub 断连或 Redis 不可用时，订阅退化为**纯 TTL（现状）**：漂移窗口
  回到上表最坏值，不会比没有通道时更糟，也不引入新的拒绝面（读路径不因
  通道故障改变行为）。
- 发布失败同样只记日志、不阻断管理员写操作；此时全局行为等价于现状。
- 与 `AETHER_CONSISTENCY_FIRST` 开关正交：该开关收敛的是 Redis 故障期
  限额/ fail-open 语义（见 [redis-consistency-first](../adr/redis-consistency-first.md)），
  本通道收敛的是正常期的失效延迟；两者故障期都退化为可文档化的现状语义。
- 消息格式以 `v` 字段版本化；新增缓存域为兼容扩展，废弃域在文档标注。

## 放弃方案 B 的理由

版本戳方案要把校验放进每条读路径（认证热路径首当其冲），并为 16 处缓存
逐一定义版本粒度与 Redis 故障降级，侵入和持续维护成本都高于 pub/sub；
而失效广播只需要在既有写路径后追加一次发布、在启动时挂一个订阅任务，
读路径完全不动。方案 B 保留为“第一期落地后仍有不可接受残余窗口”时的
后续选项，届时按本 ADR 的演进条款另行评审。

## 验收

1. 按本 ADR 实现发布点与订阅点，多节点拓扑（
   [multi-node-deployment](../operations/multi-node-deployment.md) 的
   standalone compose）下双进程互发失效：A 节点执行禁用 API key / 修改
   系统配置 / 添加 IP 封禁，B 节点在消息投递后下一次读即回源，不再等
   TTL；本实例自发的消息不触发重复失效。
2. 订阅断开期间：B 节点行为退化为 TTL-only，与无通道基线一致；重连后
   恢复广播失效。
3. 消息格式版本化测试：未知 `v` 被忽略并计数，不 panic、不影响既有域。
4. 第一期未接入的缓存域（provider catalog 等）行为不变，回归基线通过。
