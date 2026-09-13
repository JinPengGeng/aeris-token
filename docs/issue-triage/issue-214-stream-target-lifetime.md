# #405：direct passthrough 流的 target 许可生命周期

父项：[Issue #214](https://github.com/JinPengGeng/aeris-token/issues/214)。
实施子项：[Issue #405](https://github.com/JinPengGeng/aeris-token/issues/405)。

## 问题与决策

`DirectPassthroughInlineBodyState::prepare_client_chunk_yield` 原先在首个客户端 chunk 时释放 target 许可。首字节只表示响应开始，慢上游仍占有连接；提前归还会让后续同 target 请求越过已配置的容量限制。stream pump 已保留许可至流结束，两条路径语义不一致。

本次保留现有 admission gate、finalizer 和可靠 usage 交付结构，仅调整 direct inline body 的释放边界：

| 状态或事件 | upstream 与 target 许可 |
| --- | --- |
| 响应未被客户端轮询、首 chunk、后续数据、客户端暂未继续读取 | body 继续拥有 upstream 和许可 |
| 已观察正常 EOF、传输错误、首字节或 idle timeout | 先销毁 upstream，再同步释放许可，然后发送可能的缓冲尾块/终态错误或进入 usage 终结 |
| 已识别 provider 错误或 native Anthropic `message_stop` | 按现有终止策略停止 upstream，在终态 chunk 返回时已释放许可 |
| body drop、读取任务取消 | 在 body 的同步 Drop 路径销毁 upstream 并归还许可，随后保留原 detached finalizer 交付 |
| usage 持久化或可靠终态 admission 有背压 | finalizer 继续等待/交付；target 许可已释放 |

统一使用 `finish_upstream`，释放操作幂等；保留 `stream_upstream_target_permit_release` 的 metric 名称，但记录实际释放时间。两个 target in-flight metric 的 HELP 文案去掉已失效的“仅首字节前”描述。未改变 metric 名称、标签或 gate 的选择和等待策略。

此处“流结束”指 body 已观察到的 upstream 结束或主动销毁资源。客户端不继续轮询时，普通 OpenAI `[DONE]` 后仍存在的 upstream 不会凭空视为 EOF；继续持有容量，直至实际读到 EOF、读超时或 body 被丢弃。native Anthropic 已有 `message_stop` 即主动销毁 upstream 的行为不变。

## 验证

定向回归使用真实 `UpstreamTargetAdmission`、实际 inline body / Axum body 和原 finalizer；测试上游通过受控异步 channel 提供字节，不连接外部服务。

- 首 chunk 后同 target 的 try-acquire 和有界 acquire 均不能越过容量，另一个 target 可用；慢客户端与正常 EOF 覆盖。
- 读错误、首字节 timeout、idle timeout 在终态 chunk 时已归还许可。
- provider 错误只转发一次；native Anthropic `message_stop` 的主动终止释放。
- 未轮询 body、首字节后 body drop 均同步释放，不依赖 detached finalizer 获得调度；读取任务 abort 后释放。
- 用真实 usage runtime 将可靠终态容量限制为 1，并以受控持久化策略阻塞唯一 slot：EOF 或下游 drop 后 target 已可用；取消客户端等待后，解除背压仍必须得到最终 usage 状态，不遗留 streaming/pending。此 fixture 仅有 usage writer，没有 settlement writer；完成请求的 billing 状态按原规则保持 pending，取消请求为 void。

本地验证命令（Rust 1.95，保留现有 PATH；完整 Gateway 回归沿用现有 CI 的 16 MiB 测试线程栈）：

```sh
rtk proxy env PATH="/Users/fengying/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin:$PATH" \
  cargo test -p aether-gateway --lib target_lifetime_tests -- --nocapture
rtk proxy env PATH="/Users/fengying/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin:$PATH" \
  RUST_MIN_STACK=16777216 cargo test -p aether-gateway --lib \
  execution_runtime::stream::execution::tests:: -- --test-threads=1
rtk proxy env PATH="/Users/fengying/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin:$PATH" \
  cargo clippy -p aether-gateway --all-targets -- -D warnings
rtk proxy env PATH="/Users/fengying/.rustup/toolchains/1.95.0-aarch64-apple-darwin/bin:$PATH" cargo fmt --all --check
rtk git diff --check
```

新增 8 项定向测试全部通过（0 failed / 0 ignored，0.11 秒）。完整 stream execution 模块 126 项全部通过（0 failed / 0 ignored，2.38 秒），包含既有 Anthropic、idle timeout、首字节持久化及取消期间可靠终态交付回归。Rust 1.95 的 gateway all-targets Clippy（`-D warnings`，4 分 39 秒）、`cargo fmt --all --check`、`git diff --check` 均通过。

实现代理完成后，主线程独立审查全部生产 diff、body/finalizer 完整上下文和八项测试，未发现阻塞项；再次实际执行八项定向测试，8 passed / 0 failed / 0 ignored（0.10 秒）。评审确认释放先于 detached finalizer 和可靠 usage 排队，helper 在 core 转移后可重复调用；没有改变计费状态机。这是代理间实现/评审分工，不代表第二位人类维护者审批。最终 PR head 的四项 GitHub required checks 仍须全部通过。

验证期间的两项调整均未改变生产行为：首次定向执行的背压测试错误地把“终态 usage 已交付”理解为“billing 也必须结算完成”，导致 1 项断言失败；核对 `lifecycle_status_and_billing` 与 fixture 的无 settlement writer 配置后，只修正断言为 completed/pending 与 cancelled/void。扩大模块回归时默认线程栈在既有 frame-stream 测试溢出，改用 `.github/workflows/rust-ci.yml` 已有 `RUST_MIN_STACK=16777216` 设置后全部通过。

## 范围、风险与回滚

本次不改变全局/分布式许可、同步和 local-tunnel 路径、调度选择、重试或计费政策。修复后首字节之后同 target 的并发会受已配置 limit 约束，这是预期行为；确需更多并发可显式调整 target limit。没有数据迁移，可 revert 本子项。

测试覆盖进程内真实 body/gate/usage runtime 的生命周期和背压，不声称完成真实公网连接、分布式 Redis 或生产负载验收。父 #214 继续开放，保留同步/tunnel 路径、Redis 错误契约、客户端依赖故障等待上界等独立残余范围。
