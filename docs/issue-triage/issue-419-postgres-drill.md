# #419 真实 PostgreSQL 暂停与恢复演练

本文件记录本地实际观测；最终 PR 的 hosted 检查须针对最终 head 单独核验。
环境为 macOS / Rust 1.95 / PostgreSQL 17.11 / Redis 8.10.1。演练只创建并
暂停自己的 PostgreSQL 集群、Redis 和 Gateway，使用从未投入生产的 fixture key，
不读取用户数据库、不调用真实 provider。脚本为 `tools/ci/run_readiness_drill.sh`。

## 失败对照

源 head `ba77faa3b1719cc4dceb3e74ae2816c6a68f56e5` 是 main `9cb3f03f` 加
#418 的标准流副本优化，不包含 #419。重新构建的 Gateway SHA256 为
`ac768bad2f15b907986841c695bb83837c6d652fcb93a1b751cadff304c9db43`。
北京时间 2026-09-14 05:45:14 启动同一增强脚本：健康依赖下新 key 连续三次
401，随后暂停整个 owned PostgreSQL 集群，第一个新 key 请求在 2.011367 秒触及
curl 的两秒硬期限，HTTP 状态 `000`；脚本按预期 exit 1。
清理记录确认 `owned_fixture_removed=true`。这是缺口的失败证据。

## 候选实现观测

北京时间 06:03:51，以 #419 工作树候选实现重新构建的 Gateway 执行完整脚本。
当时实现尚未提交，且之后仍需完成回归与集成，不能把这次候选成绩绑定到尚不存在的
最终 head。候选 binary SHA256：
`8d63978ae2bc9ff4d7ef98052364ad3105ea9417ea83278039281d40df8be3e3`。

| 阶段 | 三次公开响应 | 耗时（秒） | 本地请求许可 |
| --- | --- | --- | --- |
| 初始健康，每次新 key | 401 / 401 / 401 | 0.004094 / 0.002054 / 0.003070 | limit 8 / in_flight 0 / available 8 |
| PostgreSQL 仍暂停，每次新 key | 502 / 502 / 502 | 1.004164 / 1.005759 / 1.004956 | limit 8 / in_flight 0 / available 8 |
| PostgreSQL 恢复，每次新 key | 401 / 401 / 401 | 0.005312 / 0.003486 / 0.003481 | limit 8 / in_flight 0 / available 8 |

三次 502 均包含原 trace、`error.code=control_unavailable`、`retryable=true`、
`failover_disposition=retry_request`、`Retry-After: 1` 和固定安全消息
`gateway control unavailable`。脚本校验没有内部数据库/Redis 地址或错误详情。
暂停期间 liveness 仍为 200。恢复请求换用新 key，避免已有无效凭据负缓存掩盖数据库读路径。

原有 PostgreSQL 停止/重启、Redis 停服及暂停各十次 503、恢复各连续三次 401、
两个依赖同时暂停、SIGTERM 摘流和独立 liveness 阶段均通过；脚本 exit 0，owned fixture
已删除。8/0/8 是本地请求许可快照；分布式恢复由真实 Redis 准入请求证明，未声称读取了
所有 Redis lease 或证明生产多节点容量。原始候选证据在本机
`/private/tmp/aeris-419-candidate-0604`，本文件保留跨电脑可见的关键数值。

## 复现

```sh
cargo build --locked -p aether-gateway --bin aether-gateway
AETHER_GATEWAY_BIN="$PWD/target/debug/aether-gateway" \
  bash tools/ci/run_readiness_drill.sh
```

PATH 需有 PostgreSQL、Redis、Node、curl、jq；使用 `CARGO_TARGET_DIR` 时对应调整
binary 路径。脚本记录 binary hash、服务版本、逐次响应和清理结果，并拒绝覆盖旧证据目录。
生产默认阶段期限为 30000ms；演练用专属 fixture 的 1000ms。此证据不替代生产部署验收，
不把控制上下文期限扩展为管理员事务、上游执行或响应流总期限。
