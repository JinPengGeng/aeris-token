# #410：限制 Redis managed connection 的重连退避

父项 [#214](https://github.com/JinPengGeng/aeris-token/issues/214)，解除 [#409](https://github.com/JinPengGeng/aeris-token/issues/409) 真实请求恢复演练的阻塞。

## 实际缺陷与原因

#409 对真实 gateway 启用 distributed request gate，在 Redis 停止期间发送十次请求。请求均按 250ms 配置安全返回 503；但 Redis 重启且独立 RESP PING 正常后，gateway readiness 的 runtime PING 持续 timeout，超过原有 30 秒恢复期限。没有改变原测试期限来绕过失败。

当前 lockfile 使用 redis 0.28.2 和 backon 1.6.0。`connection_manager_config` 只设置 connection timeout，继承 redis 默认 factor=100。redis 0.28.2 实际把该值传入 `ExponentialBuilder::with_factor`；backon 默认 min delay=1 秒、max delay=60 秒，再加 0–100% jitter。第二次失败后的等待因此直接达到 60–120 秒。

此前立即重启测试通常在第一次 1–2 秒 backoff 内恢复，不能暴露这个情况。新测试保持隔离 Redis 不可用至少 3 秒，期间继续发失败命令，再验证同一个 RuntimeState 的 PING、KV 及 semaphore 获取/释放恢复。旧配置真实失败：0 passed / 1 failed / 0 ignored，9.29 秒；日志确认已启动真实隔离 Redis，不是可选跳过。

## 实现与取舍

在既有 managed fast、stream、admin lane 入口显式设置 `factor=2`、`max_delay=2000ms`，保留现有重试次数、jitter、connection timeout 和共享 connection manager。base delay 最高 2 秒；当前 backon 的 jitter 可再加最多约 2 秒，不能把 API 名称 `max_delay` 描述成包含 jitter 的 2 秒硬期限。

长故障下会比旧配置更频繁地尝试连接；共享 lane 和 jitter 避免按每个 HTTP 请求单独重连及完全同步重试。此选择修正短故障恢复拖到分钟级的问题，同时不使用忙循环。连接尝试本身仍受既有 deadline 约束；调度、连接耗时和 lease 收敛不属于单个 backoff 上界。

没有增加应用命令重放、本地 fail-open、故障期间容量绕过，也不改变 Redis 切主/丢数据、token 所有权或账务幂等政策。独占 blocking stream/usage cleanup 连接继续沿用原有生命周期。

## 验证与恢复

```sh
AETHER_REQUIRE_LOCAL_REDIS_TESTS=1 cargo test -p aether-runtime-state redis_connection_manager_recovers_after_sustained_outage -- --nocapture
bash tools/ci/run_redis_live_tests.sh
cargo clippy -p aether-runtime-state --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

本地使用 Rust 1.95 / Redis 8.10.1。旧实现失败后，修改配置的新测试通过：1 passed / 0 failed / 0 ignored，5.45 秒。完整 strict Redis harness 通过：130 passed / 0 failed / 1 个既有 benchmark ignored，23.19 秒；缺 binary、启动失败必失败及 optional skip 契约均通过，完整真实测试运行没有跳过。crate 全 targets Clippy（`-D warnings`）、fmt、diff 检查通过。

2026-09-14 03:52 北京时间，以本分支运行时补丁构建真实 Gateway，执行 #409 的测试脚本：Redis stop/pause 各十次有界 503、重启/恢复后各连续三次 401，以及原有数据库故障、联合暂停、readiness、独立 liveness、SIGTERM 摘流/退出全部通过。此次是两项未合并代码的联合本地验证；#409 单独 PR 依赖本修复进入基线，hosted checks 尚待 PR 收集。二进制 SHA-256 为 `4e0608384dad190e42486ea0d45a55b907f5a236be66c075e22e1345c28244f2`，PostgreSQL 17.11，Redis 8.10.1。

独立 reviewer 已核验完整三文件差异及锁定依赖源码，接受 factor/jitter 语义、共享 manager、无应用命令重放和真实持续故障测试；未把主线程测试结果冒充独立复跑。

回滚为 revert 两个 reconnect 配置值及对应回归；无数据迁移。回滚会重新引入长退避风险，不能据此宣称父 #214 或部署级恢复全部完成。
