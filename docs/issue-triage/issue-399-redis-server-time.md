# Issue #399：Redis 分布式租约时间源

关联：[#399](https://github.com/JinPengGeng/aeris-token/issues/399)、父项 [#211](https://github.com/JinPengGeng/aeris-token/issues/211)。

## 问题与决策

原 acquire、renew、snapshot 使用网关 `unix_time_ms()` 计算有序集合的过期边界。不同网关时钟偏差会提前剔除有效租约或保留已过期租约，破坏共享容量约束。

三种操作现在复用同一个 Redis Lua 脚本，在一次原子执行内调用 `TIME`，按 `seconds * 1000 + floor(microseconds / 1000)` 计算毫秒时间。获取和续租以同一次 TIME 结果计算截止时间；清理包含等于当前时间的成员。客户端只传 TTL、容量和 token，不传客户端时间或预计算截止时间。

保留 token 所有权、饱和返回值、失效 token 不可续租、键的相对 PEXPIRE、release 幂等性及 Unavailable 失败封闭语义。TIME 必须位于第一处写操作之前；ACL 禁止 TIME 时 acquire、renew、snapshot 均返回 Unavailable，不清理或改写已有租约，也不回退本地时间或内存门禁。release 不需要 TIME 权限，便于故障清理。续租失败仍将 permit 标记为 unhealthy。

Lua number 和 ZSET score 使用双精度数，精确整数上限是 `2^53 - 1`。Rust 拒绝零值或超过该上限的 TTL，脚本在写入前拒绝 `server_now + TTL` 超出上限；移除原来的 `u64 as i64` 截断转换。静态非法 TTL 返回 InvalidConfiguration，依赖当前服务器时间的溢出拒绝按 Redis 执行失败映射为 Unavailable。

## 兼容与边界

- 部署基线是仓库固定的 Redis 7.4.11；Redis 7 的脚本使用 effects replication，允许 TIME 和写命令在同一 Lua 执行。无需提升当前部署版本要求。
- Redis ACL 必须允许 `TIME`，以及已有脚本和 ZSET/PEXPIRE 操作。缺少 TIME 权限会显式拒绝请求，运维应先配置权限再滚动升级。
- Redis TIME 是主节点墙上时间；本改动消除网关之间的时间偏差，不解决 Redis 主节点时钟跳变、异步复制丢失租约或切主期间的数据一致性。旧客户端仍可能写入偏移时间，应尽快完成所有网关的滚动升级。
- 不修改 memory backend、streaming EOF、Retry-After 或调度器 #52。
- 回滚只涉及代码；键名、token、score 毫秒格式、TTL 均保持兼容。回滚会恢复客户端时钟风险。

## 验证

新增真实 Redis 契约测试：四个 runtime、64 个并发申请、容量 8；跨实例 token 释放；续租、真实过期与迟到续租；TIME ACL 拒绝后的集合不变和 permit unhealthy；Redis 停机、重连和旧 token 不可复活；TTL 边界。

时钟偏差测试在单线程测试运行器中替换网关时钟为 0 和服务端时间加 24 小时，实际调用生产 Redis runner，并检查服务端 TIME 前后范围内的租约 score。三个操作分别验证快时钟不能移除有效租约、慢时钟不能保留或复活过期租约。测试时钟替换仅在 cfg(test) 编译，不改变宿主机时间，不影响其他测试线程。

2026-09-14 本地验证：Rust/Cargo 1.95.0（仓库固定工具链）、macOS arm64、Redis 8.10.1。

| 命令 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy -p aether-runtime-state --all-targets -- -D warnings` | 通过 |
| `AETHER_REQUIRE_LOCAL_REDIS_TESTS=1 cargo test -p aether-runtime-state --lib semaphore_server_time -- --nocapture --test-threads=1` | 6 个新增契约测试通过 |
| `bash tools/ci/run_redis_live_tests.sh` | 129 passed、0 failed、1 个既有性能 benchmark ignored；缺二进制/启动失败的强制失败检查及可选本地跳过检查全部通过 |
| `git diff --check` | 通过 |

本机默认 PATH 指向 Homebrew Rust 1.98.0，会使既有跨 crate Clippy 规则报错；验证已显式将 `/Users/fengying/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin` 置于 PATH 首位，使用仓库要求的 1.95.0。没有修改旁支代码来适配未指定的新工具链。

严格 Redis harness 的本地证据在 `artifacts/redis-runtime/`；该目录不进入版本控制，表格保留可在其他电脑或 GitHub CI 复跑的命令。

另从 `https://download.redis.io/releases/redis-7.4.11.tar.gz` 构建与生产相同版本的 Redis 7.4.11（macOS、`make -j 2 redis-server MALLOC=libc BUILD_TLS=no`），用 `AETHER_REDIS_SERVER_BIN=<Redis 7.4.11 redis-server 的绝对路径>` 运行上述 6 个新增契约测试，结果 6 passed、0 failed。此项包含实际 Redis 7.4.11 的 TIME ACL 拒绝、时钟偏差、并发和故障恢复，并非仅依据文档推断兼容性。

主线程独立评审已读取三处调用、Lua 状态转换、permit renewal/release 调用方和全部新增用例，未发现阻塞问题；独立重跑 6 项严格真实 Redis 用例，6 passed、0 failed、0 ignored，耗时 2.94 秒。TIME 拒绝发生在首个写命令前，旧 token 不可复活，错误映射和释放路径保留。

本分支实现、本地验证与独立评审已完成；下一步是 fork PR、四项受保护 GitHub checks 及合并。尚未做真实生产切主或墙上时钟跳变演练，前述边界保持开放。
