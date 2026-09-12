# #229 开发者体验验收记录

日期：2026-09-13。基线：fork `main` 的 `f663b48a90d65c148b827506cc8a4cb17c85ebba`。范围仅为 [#229](https://github.com/JinPengGeng/aeris-token/issues/229)，不修改上游。

## 准确性、收益与规划

原报告的双白名单和文档入口缺口成立。PR [#282](https://github.com/JinPengGeng/aeris-token/pull/282) 已合入贡献指南、固定工具链和分级检查；PR [#304](https://github.com/JinPengGeng/aeris-token/pull/304) 已合入唯一函数名称声明和 engine/admin 侧的一致性测试。维护者于 2026-09-11/12 将剩余规划收敛为 P2/S 的白名单一致性工作，明确避免大范围命名清理。本次同时补齐原报告中仍缺少的任务导航、模型配置解释和架构测试入口，使原报告各项都有明确处理结果。

收益为减少新增内置函数时的 admin/engine 漂移，以及降低贡献者定位模块和测试的成本；没有证据把本问题提升为 P1。复杂度 S，风险低：保留公开函数常量和 helper 接口、API/SQL 字段名称与现有函数行为，仅将不一致时的 panic 改为现有 `Unsupported` 错误，并加强测试及文档。

## 验收与决定

| 原报告项 | 当前证据与处理 | 验收结论 |
| --- | --- | --- |
| engine/admin 双白名单 | `aether-billing::FORMULA_ALLOWED_FUNCTIONS` 是唯一允许名称列表；`is_formula_function_allowed` 同时用于 evaluator 和 admin 校验；`rules.rs` 的写入归一化路径调用 admin 校验 | #304 已交付；新增函数不需要修改 admin 列表 |
| 函数同步测试 | billing 测试遍历导出列表并要求每个名称有正确数值 fixture，另覆盖缺少参数和非法函数；gateway 测试遍历同一列表并拒绝非法调用 | 当前函数行为及两侧准入有明确验收；函数声明仍需对应实现与 fixture |
| #304 review 的 panic 风险 | `evaluate_function` 的未实现 fallback 改为 `UnsafeExpressionError::Unsupported` | 将未来声明/实现不一致转换为求值错误；不改变现有六个函数 |
| 本地反馈无分级 | #282 的 CONTRIBUTING 已给包级/工作区/真实数据库检查；[module-map](../module-map.md) 增加实际公式过滤器和冷缓存依赖说明 | 提供最小反馈路径；2m34s 是旧环境观测值，不承诺固定时间或宣称依赖解耦完成 |
| 任务导航缺失 | 新增 `docs/module-map.md`，从 CONTRIBUTING 链接到代码、数据与测试入口 | 新贡献者有可发现的入口；模块迁移时由同一 PR 维护 |
| 模型映射“是数据不是代码”缺少说明 | module-map 描述已有协议下通过管理员界面/API 修改映射，读取、验证、模型测试及恢复原配置 | 解释配置与代码变更的边界，不直接修改部署数据 |
| `provider_model_mappings` / `model_provider_model_mappings` 命名不同 | 候选 SQL 明确使用 `m.provider_model_mappings AS model_provider_model_mappings`；前者是模型 JSON 字段，后者是候选投影别名 | 保留兼容接口并文档化两个名称及搜索方式；拒绝没有额外收益的数据库/API 重命名 |
| 跨 crate 源码字符串测试不可见 | module-map 链接 `apps/aether-gateway/src/tests/architecture`，说明 SQL/Redis/body/shim 约束、过滤命令和调整规则时的评审要求 | 显示规则入口，不删除约束或以字符串伪装通过测试 |

## 验证记录

基线 PR #304 的 GitHub `Test (Gateway)` 和 `Test (Workspace Rest)` 均为 success，检查 run 为 [34673325699](https://github.com/JinPengGeng/aeris-token/actions/runs/34673325699)。这是历史实现的验证，不代替本次修改后的测试。

本次验证：

- `cargo test -p aether-billing --lib formula_engine::tests`：12 passed，0 failed，0 ignored；包含每个允许函数的数值结果、缺少参数、非法函数及已有非有限值/维度/定价边界。
- `rustfmt --check --edition 2021 crates/aether-billing/src/formula_engine.rs`：通过。
- 四个改动 Markdown 文件的本地链接检查：33 个链接全部可解析到现有路径；`git diff --check` 通过。
- `RUST_MIN_STACK=16777216 cargo test -p aether-gateway --lib handlers::admin::billing::tests`：2 passed，0 failed，0 ignored；确认 admin 接受所有共享函数并拒绝列表外函数。

Rust 使用仓库固定的 1.95.0；本机通过该 toolchain 的绝对路径调用 cargo/rustc，因为默认 PATH 没有 rustup。文档中的命令面向按 CONTRIBUTING 安装工具链的开发环境。

## 评审与状态流转

本次变更先交维护者复核 diff、测试和这份逐项结论，再建立关联 `Closes #229` 的 fork PR。合并前以当前 PR head 的四个 required checks 为准；合并后由 GitHub 关闭 Issue，再同步 Project 为 Done。此记录准备完成不等于远端已关闭，不应据此提前改写看板状态。

回滚使用 PR revert，恢复本次错误 fallback、测试和文档；不需要数据迁移。#226 的其他计费设计/依赖残项继续由 #226 自身验收，不能据 #229 的完成一并关闭。
