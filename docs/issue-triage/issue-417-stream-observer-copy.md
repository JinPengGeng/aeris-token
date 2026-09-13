# #417：移除标准流观测前的临时副本

父项 [#215](https://github.com/JinPengGeng/aeris-token/issues/215)，基线 `36fe372f8cc73cdd33e6574f754ea0a47c618a2f`。`observe_stream_chunk` 在无需私有协议 normalizer 时也先 `chunk.to_vec()`，仅用于下一次同步借用；该分配和复制不承担额外所有权职责。

沿用同一执行模块 `decode_response_body_bytes` 中的 `Cow` 模式：标准 chunk 使用借用，normalizer 的输出保留 owned Vec。`observe_normalized_bytes` 仍同步按行缓冲，维持原 1 MiB 行长限制、解析错误和终态处理。这里没有修改 SSE JSON 预过滤、normalizer 实现、usage/计费或公共响应。

验证使用 Rust 1.95 和 `RUST_MIN_STACK=16777216`，运行现有 `execution_runtime::stream_pump::tests`，并做 Gateway 全 targets Clippy、fmt、diff 检查。没有添加仅镜像 `Cow` 分支的测试，也不通过微小改动推导未测量的吞吐比例。最终结果和独立评审记录在关联 PR，四项 required checks 是合并门禁。

```sh
cargo test --locked -p aether-gateway --lib execution_runtime::stream_pump::tests -- --test-threads=4
cargo clippy --locked -p aether-gateway --all-targets -- -D warnings
cargo fmt --all --check
git diff --check
```

本次只移除进入观察器前的临时副本，按行缓冲、私有协议转换及其他层的副本仍存在，不能称为整条链路零拷贝；父 #215 继续开放。无依赖或数据迁移，revert 本切片可回滚。

2026-09-14 本地验证：现有 stream_pump 模块 **12 passed / 0 failed / 0 ignored**，0.40 秒；Gateway 全 targets Clippy（`-D warnings`）通过，fmt/diff 通过。独立静态评审 **Accepted**：确认借用仅活在同步调用中，行缓冲按原逻辑复制所需字节，normalizer 的 owned 输出和错误处理等价。评审没有独立复跑 Cargo 或服务。最终 GitHub head 的四项 required checks 仍须通过。
