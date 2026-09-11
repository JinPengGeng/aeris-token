# #92 watchdog 健康反馈修复

基线：`12a1d265c`；日期：2026-09-12。当前客户端取消已有 AttemptCancellationGuard，本次只修复候选 watchdog 首字节超时绕过运行时失败报告后缺少 key/pool 反馈的问题。

## 实现与副作用

- helper 始终报告 TransportTimeout；调用点先复用 PoolStreamTimeout 和分类后的 HealthFailure，再按既有 transport policy 选择 candidate retry 或 504 stop。
- 不发 AttemptFailure，不引入额外亲和删除或更换 failover 策略。
- deadline 优先的 biased select 在内外 timer 同时 ready 时取得超时所有权；如果 inner 已标记 terminal_started，则等待其现有终态处理，不重复结算。
- abandoned 标记阻止被丢弃 inner future 的取消 guard 再结算；真正客户端取消的现有路径保留。
- upstream permit 的释放/response 持有逻辑不变；无数据库迁移、配置或公开 API 变更。

## 验证与评审

实现由独立运行时 agent 完成，主线程复核 helper/port 分支、effect 类型、terminal/abandoned 门控以及测试断言。新测试使用本地 loopback 上游与内存 repository，不依赖真实 provider、Postgres 或 Redis；fixture 在测试线程中延迟 poll，使两个 timer 同时 ready，以确定性覆盖竞态。生产代码不做 busy wait。

- 首次 targeted test：1 passed，0 failed，执行0.96s。
- 最终 `cargo test --locked -p aether-gateway --lib stream_candidate_watchdog_ -- --nocapture`：15 passed，0 failed，0 ignored，5396 filtered，执行0.98s。包含 retry 与 stop 两个 port 级 exact-once 测试，以及 admission、fresh budget、terminalization、错误分类现存测试。
- 两个新测试均断言 key consecutive_failures=1、pool failure_count=1、Cooldown 与 stream_timeout 原因；stop 额外断言504，retry断言Candidate scope。
- Rust1.95.0；`RUST_MIN_STACK=16777216`、`CARGO_BUILD_JOBS=4`、dev/test debug=0。`cargo fmt --all -- --check`、`git diff --check` 和 `cargo clippy --locked -p aether-gateway --lib -- -D warnings` 通过。

当前是本地验证记录；最终合并仍要求 fork PR 的 Rust CI、Frontend CI 与 Automation Policy checks 通过。若出现回归，revert 本修复提交可恢复旧调度行为，代价是恢复超时健康反馈缺失。多进程实流量验证不在这组内存回归覆盖范围内。
