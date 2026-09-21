# Gateway 多实例缓存一致性

状态：Accepted（2026-09-21；基于当前源码的行为合同）

Gateway 的 PostgreSQL 持久数据是认证、provider catalog、用户设置与系统配置的权威来源；Redis/runtime-state 承担共享的安全规则等运行时状态，不承担各进程本地缓存的广播失效。写入路径清除或更新**当前实例**的相应缓存；其他实例依靠有限 TTL 再读取权威状态。不能据此承诺跨实例立即失效。直接改库也不会触发本地失效。

| 代表性缓存 | 当前跨实例可见性窗口及源码 | 本实例写后处理 |
| --- | --- | --- |
| 认证上下文/API key 快照 | 正向上下文固定 60 秒；命中后的强读复核间隔默认 10 秒，环境变量可调低，代码限定最长 10 秒；负向命中默认 10 秒、可配置（无相同的 10 秒上限）；本地快照和数据层快照各固定 30 秒，强读绕过数据层快照。[上下文](../../apps/aether-gateway/src/control/auth/resolution.rs)、[本地快照](../../apps/aether-gateway/src/state/runtime/auth/api_keys.rs)、[数据层](../../apps/aether-gateway/src/data/state/auth_api_key_cache.rs) | key、用户、钱包等写路径调用 [`invalidate_auth_context_cache`](../../apps/aether-gateway/src/state/core.rs)，清空两层快照及关联授权缓存。 |
| Provider catalog | 数据层读缓存 5 秒；路由相关的其他本地缓存有各自 TTL，不能把 5 秒当作所有路由结果的统一上界。[读缓存](../../apps/aether-gateway/src/data/state/provider_catalog_cache.rs)、[路由失效](../../apps/aether-gateway/src/state/core.rs) | catalog 写路径清理本地读缓存和路由缓存。 |
| 系统配置 | 新鲜期 30 秒，后台刷新失败时最多返回总年龄 5 分钟的旧值，之后同步读权威源；授权敏感调用可用 strong 读。[读取与写入](../../apps/aether-gateway/src/state/core.rs) | 写入发布本地值或按 key 失效，并清理关联授权/调度缓存。 |
| IP 黑白名单 | 共享 runtime-state 上的本地读缓存默认 1 秒，可配置至最多 30 秒；Redis 状态变动不会自动推送给其他实例。[安全规则](../../apps/aether-gateway/src/state/runtime/security.rs) | 当前实例添加/移除时更新或清除本地缓存。 |
| 用户设置与用户群组 | 本地 JSON 设置及用户群组各 30 秒。[设置](../../apps/aether-gateway/src/state/runtime/auth/user_provisioning.rs)、[群组](../../apps/aether-gateway/src/state/runtime/auth/user_lifecycle.rs) | 用户写路径清理相关缓存并失效认证上下文。 |

关键授权在认证上下文到期前仍执行周期性安全复核；复核用 `read_auth_api_key_snapshot_*_strong` 绕过数据层快照，并通过 `resolve_wallet_auth_gate_uncached` 读取钱包授权状态。[复核入口](../../apps/aether-gateway/src/control/auth/resolution.rs)、[钱包强读](../../apps/aether-gateway/src/wallet_runtime/access.rs)。因此密钥撤销等认证状态在其他实例的普通 HTTP 缓存命中路径默认约 10 秒后需要重新复核，配置只能缩短这一间隔；后端读取失败时不沿用过期的允许结果。这是命中后的复核间隔，不承诺请求恰好在 10 秒内发生或在途请求被取消，也不替代每次发送前已有的授权检查。系统配置后台刷新失败可达到 5 分钟，运营时应按具体配置项评估能否接受这一边界。

本决策只确认上述有限陈旧和关键授权强读的现状。若需要跨实例即时撤销，或要求任意系统配置变更立即全局可见，需要另行定义并实现通知/版本检查机制及故障语义；本 ADR 不声称已有广播、全局失效或生产多实例演练。
