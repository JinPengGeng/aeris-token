# #409：Redis 故障下的公开请求准入与恢复

父项 [#214](https://github.com/JinPengGeng/aeris-token/issues/214)。本子项补真实 gateway HTTP 请求的分布式准入验收，不改变生产降级政策。

## 决定与范围

此前 `run_readiness_drill.sh` 证明依赖故障会摘除 readiness，但没有启用 shared request gate 并请求公开 API。runtime semaphore 单元测试与 readiness PING 不能替代请求路径恢复证据。

分布式并发许可是显式容量保证：Redis 不可用时不得改为本地放行。部分 RPM/缓存的 best-effort 行为有不同承诺，不能仅因为共用 Redis 而统一成 fail-open。本次只验证现有 request admission 的失败封闭和恢复；`Unavailable` 与 `Saturated` 目前都使用安全的公开 503 envelope，未扩展新的公开 error code。

复用既有 required Gateway CI 中的自建 PostgreSQL、Redis、真实 gateway 和证据 artifact：

- fixture 开启 distributed request limit=2，lease TTL=2000ms、renew=500ms、command timeout=250ms；本地 request gate 仍为 8。
- 向实际 `/v1/chat/completions` 发携带固定无效 API Key 的请求：Redis 正常时通过准入后得到 401；Redis 停止或暂停时，在鉴权之前有界得到 503。该 key 是 fixture 常量，不是真实凭据。
- 两种 Redis 故障各发送十次请求，超过本地 gate 容量；恢复后须重新连续得到 401。初始及两次恢复后还须读取既有 `/_gateway/health`，有界断言本地 limit=8、in_flight=0、available_permits=8，证明全部本地许可归还。
- 检查 OpenAI error type、trace header、`Retry-After: 1` 和无内部 gate/连接信息；记录每次 HTTP 状态及耗时。
- 恢复允许短暂 503 收敛：超时的 Redis acquire 命令可能在服务恢复后才执行，形成短期 orphan lease；必须等 lease 过期后恢复可用。readiness 成功不等于 shared capacity 已恢复。
- 保留原有 PostgreSQL/Redis 故障、两依赖同时无响应、独立 liveness 和 SIGTERM 摘流/退出全部阶段。沿用 owned fixture 清理，不访问外部数据库、Redis 或提供商。

## 验证

```sh
cargo build --locked -p aether-gateway --bin aether-gateway
AETHER_GATEWAY_BIN="$PWD/target/debug/aether-gateway" bash tools/ci/run_readiness_drill.sh
shellcheck tools/ci/run_readiness_drill.sh
bash -n tools/ci/run_readiness_drill.sh
git diff --check
```

需要本机提供 PostgreSQL、Redis、Node、curl 和 jq。脚本生成独立 fixture 和 evidence 目录，结束时只清理本次生成的数据库/进程，保留 evidence。Cargo 使用 Rust 1.95。ShellCheck、bash 语法、diff 检查通过。

首次演练在初始健康阶段失败：完全不提供 Authorization 时，当前执行路径返回 `503/server_error` 和 `missing_auth_context`，不能作为 401 恢复基线。检查实际响应和日志后，将探针改为固定无效 API Key，以明确进入现有鉴权拒绝路径；没有放松恢复必须为 401 的断言。缺认证上下文的 503 分类属于父 #254 的独立契约缺口，不在本测试改动中修复。首次失败 fixture 已清理，证据仍保留。

第二次真实演练发现 Redis 重启并通过独立 PING 后，Gateway readiness 仍超过原有 30 秒期限。根因为锁定 redis/backon 组合继承的 factor=100 会把第二次重连等待推到 60–120 秒；另建 [#410](https://github.com/JinPengGeng/aeris-token/issues/410) 修复，没有放宽恢复期限。本 PR 的运行时前置是 #410，详见[重连决定与验证](issue-410-redis-reconnect.md)。

2026-09-14 03:52 北京时间，使用含 #410 补丁的真实 Gateway 运行初版脚本全部通过：初始三次 401；Redis stop/pause 各十次 503；恢复后各三次 401；原 PostgreSQL 停止/暂停、两依赖联合暂停、readiness 恢复、独立 liveness、SIGTERM 摘流/退出全部通过。所有 HTTP 观测均写入脚本 evidence，owned fixture 已清理。环境为 PostgreSQL 17.11、Redis 8.10.1；Gateway SHA-256 为 `4e0608384dad190e42486ea0d45a55b907f5a236be66c075e22e1345c28244f2`。这是 #409 脚本与 #410 运行时补丁的联合本地验收。

独立评审指出，串行 401 只能排除所有本地许可耗尽，不能排除部分永久泄漏；已补齐既有健康端点的完整本地容量断言，不引入新运行时接口。增强后的完整真实演练通过，初始、Redis 重启后、Redis 恢复响应后三个阶段均读回本地 limit=8、in_flight=0、available_permits=8；所有原阶段继续通过。Redis 停止/暂停时十次公开请求最慢分别为 255.443ms / 257.797ms；暂停恢复后先有 23 次 503 等待孤立租约收敛，随后连续三次 401。修订稿独立复审已接受，评审核验脚本与保存的真实证据，独立运行 ShellCheck、bash 语法和 diff 检查；没有把该复审描述为再次运行完整演练。

GitHub hosted 的 [Rust CI run 34779505761](https://github.com/JinPengGeng/aeris-token/actions/runs/34779505761) 在 head `37cf19fa35916650b086543f3b95da34f3fbb67d` 全部成功。主线程下载并核验 `readiness-withdrawal-recovery` artifact（ID `10324946850`）：Ubuntu、PostgreSQL 16.15、Redis 7.0.15，Gateway SHA-256 `e91e770fbc73549521af24e3bb7a21d7a9f2c7e7d767066ce40e7f51392ce961`；所有演练阶段通过，三个本地容量快照精确为 8/0/8，cleanup 为 exit=0、removed=true。[独立复审记录](https://github.com/JinPengGeng/aeris-token/pull/413#issuecomment-5655905838)和[hosted 证据核验](https://github.com/JinPengGeng/aeris-token/pull/413#issuecomment-5655942011)已保存在 PR。

#410 已通过 [PR #411](https://github.com/JinPengGeng/aeris-token/pull/411) 合入主干 `44bd242e15572e17cb808d2b9db213a443ab16cd`；本分支已同步该基线，最终变更仅为此文档与演练脚本。同步后的最终 head 必须重新通过四项 required checks；先前 hosted 结果只证明上述明确的 head，最终运行和合并状态以 PR checks 为准。

## 剩余边界与回滚

这不是 authenticated upstream、付费请求、在途流 lease 丢失、RPM 降级或生产多节点容量验收；也不证明默认 30 秒 lease 参数下的恢复时间。本次 401 探针避免配置/调用真实提供商，只证明全局 admission 与原鉴权路径恢复。

## Admission retry hint follow-up

The public overload response is shared by local saturation and distributed
Redis admission failure. Both are transient capacity outcomes, so the gateway
now emits `Retry-After: 1` alongside the existing `503` envelope. This is a
client-facing hint only: it does not change the distributed gate's fail-closed
policy, lease TTL, or recovery behavior. Clients should still apply bounded
backoff and honor repeated failures rather than retrying without a limit.

The response contract test in `apps/aether-gateway/src/api/response.rs` covers
the header for the shared builder; the live Redis stop/pause evidence above
continues to validate the admission status and recovery path. The existing
fixture must be refreshed on the next hosted drill so the artifact records the
header from the final binary.

父 #214 保留各自残余。无运行时行为或数据迁移，回滚为 revert 测试与文档。
