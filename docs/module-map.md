# 任务与模块导航

本文供贡献者从任务定位现有入口；模块移动、接口或测试命令变化时，由对应 PR 同步维护。工具链、数据库前置条件和提交前检查见 [贡献指南](../CONTRIBUTING.md)，PR 门禁见 [开发工作流](development-workflow.md)。

## 从任务找代码

| 任务 | 首先查看 | 验证入口 |
| --- | --- | --- |
| 计费公式、维度和内置函数 | [formula_engine.rs](../crates/aether-billing/src/formula_engine.rs)、[admin 公式校验](../apps/aether-gateway/src/handlers/admin/billing/mod.rs) | `cargo test -p aether-billing --lib formula_engine::tests`；admin 侧命令见下文 |
| 模型别名与提供商模型配置 | [ModelAliasDialog.vue](../frontend/src/features/providers/components/ModelAliasDialog.vue)、[模型 API](../frontend/src/api/endpoints/models.ts)、[模型数据契约](../crates/aether-data/contracts/src/repository/global_models/types.rs) | [ModelMappingDialog 测试](../frontend/src/features/providers/components/__tests__/ModelMappingDialog.spec.ts)、[模型测试请求](../frontend/src/features/providers/components/provider-tabs/__tests__/model-test-request.spec.ts) |
| 模型候选与路由 | [PostgreSQL 候选读取](../crates/aether-data/adapters/postgres/src/candidate_selection.rs)、[路由核心](../crates/aether-routing-core/src) | `cargo test -p aether-data-postgres parse_provider_model_mappings`；`cargo test -p aether-routing-core` |
| 持久化与仓储 | [数据契约](../crates/aether-data/contracts/src/repository)、[运行时仓储](../crates/aether-data/runtime/src/repository)、[PostgreSQL 适配器](../crates/aether-data/adapters/postgres/src) | 对应包内测试；真实数据库验收按贡献指南中的隔离 PostgreSQL harness 执行 |
| Redis 状态与锁 | [runtime-state](../crates/aether-runtime/state/src)、[Redis 所有权测试](../apps/aether-gateway/src/tests/architecture/runtime_and_security.rs) | `cargo test -p aether-runtime-state`；跨模块改动另跑架构测试 |
| 协议转换与提供商请求 | [AI formats](../crates/aether-ai/formats/src)、[provider transport](../crates/aether-provider/transport/src) | 对应 Cargo package 的单元测试，再按调用方补 gateway/integration 测试 |
| GitHub 协作与 CI | [开发工作流](development-workflow.md)、[工作流目录](../.github/workflows)、[自动化测试](../.github/automation/test) | 修改的工作流及其现有合同测试；四个 required checks 由 PR 核验 |

## 分级本地反馈

1. 文档或配置数据：检查链接、字段与调用路径；通过管理员界面/API 修改实例数据，无需为一个新别名重新编译网关。
2. 单个 Rust 模块：先运行包内测试过滤器，再运行包级 check/clippy。包级命令仍编译依赖；`aether-billing` 当前依赖 `aether-usage-runtime`，会经过 data/runtime。冷缓存编译时间不能当作单个测试运行时间，也不承诺 Issue 中旧环境的 2m34s 能在所有机器复现。
3. 公共模块、依赖或跨 crate 迁移：在包级测试后运行 gateway 架构测试，再按贡献指南补适用的工作区检查。PR required checks 始终是最终合并门禁。

例如，修改公式时从以下命令开始；第二条同时验证 admin 接受共享列表、拒绝列表外函数：

```bash
cargo test -p aether-billing --lib formula_engine::tests
RUST_MIN_STACK=16777216 cargo test -p aether-gateway --lib handlers::admin::billing::tests
```

内置函数仅在 `aether_billing::FORMULA_ALLOWED_FUNCTIONS` 声明允许的名称，admin 调用同一个 `is_formula_function_allowed`。增加函数还需要实现 evaluator 分支并补正确结果、缺少参数和非法函数的测试；声明、实现与测试职责不同，不能只加名称就认为已有可执行语义。变量和维度由每条规则的 schema 定义，保持可扩展。决策见 [公式白名单](issue-triage/formula-allowlist-decision.md)。

## 模型映射是数据配置

已有协议支持下，添加提供商模型名称、别名或映射范围，应编辑提供商模型的别名配置，或使用 [模型 API](../frontend/src/api/endpoints/models.ts) 的 create/patch 接口。`provider_model_name` 是提供商主模型名；`provider_model_mappings` 保存额外映射，字段结构以 [ProviderModelMapping](../frontend/src/api/endpoints/types/provider.ts) 为准。新协议或新的匹配规则才需要实现代码并补回归测试。

操作前读取并保留原配置；写入时只修改目标模型，保留其他别名和范围。通过 GET 重新读取，核对保存的名称、优先级、API format、endpoint/operation 范围；随后用已有模型测试入口确认期望请求模型和选中的 endpoint。回滚时恢复该模型原先的字段值，不直接写 SQL 改线上数据。

下列名字对应不同层的表示，并非两张相互竞争的映射表：

| 名称 | 所在层与含义 |
| --- | --- |
| `provider_model_mappings` | 管理 API、模型仓储记录和 `models` 表的 JSON 字段 |
| `model_provider_model_mappings` | 候选选择查询通过 `m.provider_model_mappings AS model_provider_model_mappings` 产生的结果别名及候选记录字段，前缀说明值属于模型 |
| `config.model_mappings` | [全局模型映射界面](../frontend/src/features/models/components/ModelMappingsTab.vue) 使用的另一项配置；不能直接替代提供商模型的别名数组 |

定位完整路径时同时搜索前两个名称，例如 `rg -n 'provider_model_mappings|model_provider_model_mappings' crates apps frontend/src`。保留现有 API/SQL 名称可以避免无收益的兼容性迁移；不为统一拼写而重命名持久字段。

## 架构测试不是隐藏的运行时规则

规则位于 [gateway 架构测试目录](../apps/aether-gateway/src/tests/architecture)。其中部分测试读取其他 crate 的源码并匹配字符串，包级编译通过也可能因所有权或文件布局变化而失败。典型约束包括 Redis 仅由 runtime-state 持有、业务模块不直接执行 SQL、body 收集必须有上限、已移除的 shim 不得重建。

```bash
RUST_MIN_STACK=16777216 cargo test -p aether-gateway --lib tests::architecture
```

失败信息列出规则和命中的路径/模式。先阅读对应测试及其意图，再判断应修正代码归属还是有理由调整规则。若模块迁移改变了合法布局，应在同一 PR 解释新的边界并更新定位路径；不要为通过检查删除约束或制造无意义字符串。
