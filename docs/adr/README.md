# 架构决策记录

这里记录当前实现的“为什么”、兼容边界和失败处理，供贡献者与评审者复核。
维护入口见 [贡献指南](../../CONTRIBUTING.md)，代码定位见 [任务与模块导航](../module-map.md)。

| 决策 | 状态 | 范围 |
| --- | --- | --- |
| [恢复的原子性与失败语义](restore-atomicity.md) | Accepted | 认证备份、分阶段补偿、隔离恢复和部分失败 |
| [隧道版本兼容与协商](tunnel-version-compatibility.md) | Accepted | v1/v2/v3 的入口校验、v3 SETTINGS 和升级顺序 |
| [Usage core/runtime 与重试、DLQ](usage-runtime-retry-dlq.md) | Accepted | 实际分层、入队重试、消费确认、死信恢复和保留边界 |

以上记录于 2026-09-13 对 fork `JinPengGeng/aeris-token` 的已合并提交
`4a74b11ef9a4756f897a33f1ce0b18475a2f040d` 重新读取源码、测试与运维文档后建立。
`Accepted` 表示接受文中明确限定的现有行为，不代表没有残项、已演练生产环境，
也不把尚未合并的 PR #391 纳入已实现合同。每篇的“未完成范围”仍需单独验收。

使用描述性文件名作为稳定标识；不补造 Issue #235 提及但在此基线未找到的
ADR-0001 至 ADR-0044 历史，也不把旧审计中的 Proposed 机制改名后视为已接线。
历史原始发现保留在 [治理审计](../issue-triage/operations-governance-audit.md)。

修改相关路径的贡献者负责随 PR 更新对应 ADR，现有模块评审者按
[CODEOWNERS](../../.github/CODEOWNERS) 与 [开发工作流](../development-workflow.md)
审核。部署责任人负责环境选择、备份保管及现场验收；这里不新增个人授权或所有者。
行为改变时说明替代决策与迁移条件，旧决定标记 Superseded 并保留替代链接；
尚未实现的方案标记 Proposed，不能只因文档获批就宣称运行时支持。

每次维护至少复核调用入口、实际失败分支和一个相关测试；修改协议或数据语义时同时
更新兼容/回滚条款。源码链接指向同一 checkout，测试名称用于定位，不代表本次重新运行。
相关发布记录见 [CHANGELOG](../../CHANGELOG.md)。

Issue #235 的本次交付、验证与剩余验收见
[执行记录](../issue-triage/issue-235-adr-decision.md)。
