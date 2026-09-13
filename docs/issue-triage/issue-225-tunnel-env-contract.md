# Issue #225: tunnel 环境变量参考与漂移校验

## 准确性与范围

2026-09-13 在 fork `JinPengGeng/aeris-token` 的 `c860eec66`（当前 `origin/main`）
上复核。Issue 最初列出的七个 `_SECS` 环境变量名已在此前工作中更正；本次不再次声称修复这些名称。
残余问题成立：README 参数表是人工维护的子集，缺少分布式 stream admission、diagnostics、
Aether API 连接池等 clap 参数，且没有自动检查文档与实际参数的差异。

本次只处理 tunnel 的 clap 环境变量参考、README 入口和 CI 漂移校验。
没有改变配置解析、参数名、运行时默认值、兼容行为或网络行为。

## 优先级、收益与复杂度

- 判断：需要实施；父 Issue 保持 P1。运维复制不存在的变量名会被环境变量解析静默忽略。
- 收益：覆盖全部实际 clap env 参数，将默认值、单位和操作说明放在同一参考中，并阻止未同步文档的后续更名或默认值调整。
- 复杂度：S；使用已有 Rust 测试、clap 元数据及现有必需 CI，不增加依赖或 workflow。
- 风险：低；新代码仅在 `cfg(test)` 下编译，生成的文档不包含实际环境变量值。

## 设计决策

1. 直接枚举 `Config::command()`，不使用正则解析 Rust 属性。这样常量定义的 env 名、
   `default_value_t`、默认数组、CLI 显式更名和 enum 值均按 clap 实际元数据解析。
2. `ENVIRONMENT.md` 是完整生成文件。README 移除重复的手写参数表，保留运行说明，
   改为链接该参考。README 中剩余的环境变量示例也校验名称；启动入口和安装脚本的三个
   非 clap 变量按读取方明确列出，不误报为运行时参数。
3. 默认值列明确是 **clap 默认值**。`Option::None` 标记为“未设置”，与必填、零、禁用、
   运行时硬件推导区分。单位通过字段标识的明确规则及有界映射提供；新增未知字段必须补单位。
   说明与可选值来自 clap；隐藏的兼容输入也列出，并标明被忽略。
4. 普通测试只比较文件与生成结果；独立 ignored 测试只有被显式调用时才重新生成。
   生成命令在参考页顶部，维护者提交前需要审阅 diff。生成器只调用 env **名称** getter，
   不使用 clap 帮助输出中的当前 env 值，也不解析部署凭据。
5. 沿用 `.github/workflows/rust-ci.yml` 的 `Test (Workspace Rest)`：其 workspace nextest
   已包含 `aether-tunnel`。参考文件在 `apps/**` 内，现有变更分类与 push path 均会覆盖。
   无须增加并行 workflow 或新的 required check。

## 验收

- 参考生成命令通过，实际输出 **68 个 clap env 参数**，另列 3 个非 clap 启动/安装变量。
- 对同一新编译的 tunnel 测试二进制运行 `config::`：**52 passed、0 failed、1 ignored**。
  唯一 ignored 项是显式文档重新生成入口；两项常规文档校验均执行并通过。
  可复现命令：`cargo test -p aether-tunnel --bin aether-tunnel config::`。
  本机在生成命令完成编译后直接运行该产物，避免与其他并行任务争用 Cargo target 锁。
- 负向演练：把生成表的 `AETHER_TUNNEL_TCP_KEEPALIVE` 临时改回错误的 `_SECS` 名称，
  常规校验按预期退出 101，提示 `ENVIRONMENT.md has drifted`。重新生成后校验恢复通过。
- 环境隔离演练：仅给文档校验进程设置管理 token 占位符、`LOG_LEVEL=trace` 和
  `HEARTBEAT_INTERVAL=123`，两项常规文档校验仍通过，输出保持声明默认值和必填标记。
- `cargo fmt --all --check`、`git diff --check` 通过。
- 现有 CI 的 workspace nextest 命令确实包含 `aether-tunnel`；没有另外的 nextest 默认过滤
  排除这些测试。远端 required checks 与独立评审由 PR 流程继续验证。

## 父 Issue 的剩余工作

#225 继续开放。此切片不能证明 gateway 的全部运行时 env 参考、metrics 部署抓取与告警、
备份恢复和多节点部署演练、API 行为文档、ADR 历史或其他失效引用已完成。
它们需要按当前主干和各自关联 Issue 分别验收。

本次已核实 redirect 重放说明：`src/tunnel/stream_handler.rs` 的固定上限为每请求 5 MiB、
最多 1024 chunks、全局 256 MiB。README 的 5 MiB 说明正确；原 clap 隐藏兼容参数的
“不设累计大小限制”说明已过期。本次修正该 help 文案为输入被忽略、重放受固定资源上限
约束，并重新生成参考；未改变重放实现或 README 中正确的行为说明。
修正文案后，两项文档校验、`cargo fmt --all --check` 和 `git diff --check` 均通过。
