# 架构决策记录

这里记录当前实现的“为什么”、兼容边界和失败处理，供贡献者与评审者复核。
维护入口见 [贡献指南](../../CONTRIBUTING.md)，代码定位见 [任务与模块导航](../module-map.md)。

仓库保留两类互补的决策记录：本目录使用描述性文件名记录已经核验的行为合同；
[架构目录索引](../architecture/README.md)记录带编号的跨模块 ADR、实现阶段和延期范围。
编号 ADR 的状态以架构目录为准；下面的交叉索引只为让从任一入口进入的读者看到同一
状态，不把“Accepted”解释为生产演练或所有延期接线已经完成。

## 编号 ADR 交叉索引

| ADR | 状态（与架构索引一致） | 范围 |
| --- | --- | --- |
| [ADR-0044：Emergency chain domain boundary](../architecture/adr-0044-emergency-chain-domain.md) | Accepted（领域 scaffold + 管理员运维 v1） | 一次性持久化 grant 支持固定顺序同步 model-test；公共调度的 ledger/CAS 接线仍延期 |
| [ADR-0045：Signed provenance for tunnel release upgrades](../architecture/adr-0045-signed-tunnel-release-provenance.md) | Accepted（核心验签已实现） | 发布清单签名/校验；完整发布矩阵和生产轮换演练延期 |
| [ADR-0046：Bounded gateway readiness and health contract](../architecture/adr-0046-readiness-health-contract.md) | Accepted | readiness/health 合同；生产容量与告警验收延期 |
| [ADR-0050：Tunnel signing key rotation overlap](../architecture/adr-0050-tunnel-signing-key-rotation.md) | Accepted（部分实现） | key ID、有效期、重叠和撤销语义；持久化与 wire integration 延期 |
| [架构与请求数据流索引](../architecture/README.md) | 维护入口 | 编号 ADR 状态、请求数据流和维护规则 |

| 决策 | 状态 | 范围 |
| --- | --- | --- |
| [恢复的原子性与失败语义](restore-atomicity.md) | Accepted | 认证备份、分阶段补偿、隔离恢复和部分失败 |
| [隧道版本兼容与协商](tunnel-version-compatibility.md) | Accepted | v1/v2/v3 的入口校验、v3 SETTINGS 和升级顺序 |
| [Usage core/runtime 与重试、DLQ](usage-runtime-retry-dlq.md) | Accepted | 实际分层、入队重试、消费确认、死信恢复和保留边界 |
| [Gateway 多实例缓存一致性](gateway-cache-consistency.md) | Accepted | 本地失效、跨实例有限 TTL 与关键授权强读的边界 |
| [Provider API key 明文列清理](provider-api-key-plaintext-cleanup.md) | Accepted（分阶段，当前未删列） | 回填双写、读路径翻转与删列的收敛顺序 |

以上描述性记录于 2026-09-13 对 fork `JinPengGeng/aeris-token` 的已合并提交
`4a74b11ef9a4756f897a33f1ce0b18475a2f040d` 重新读取源码、测试与运维文档后建立；
编号 ADR 的后续状态以[架构索引](../architecture/README.md)及各 ADR 当前正文为准。
`Accepted` 表示接受文中明确限定的现有行为，不代表没有残项、已演练生产环境，
也不把尚未合并的 PR #391 纳入已实现合同。每篇的“未完成范围”仍需单独验收。

使用描述性文件名作为稳定标识；不补造 Issue #235 提及但在此基线未找到的
ADR-0001 至 ADR-0043 的缺失历史，也不把旧审计中的 Proposed 机制改名后视为已接线。
历史原始发现保留在 [治理审计](../issue-triage/operations-governance-audit.md)。

修改相关路径的贡献者负责随 PR 更新对应 ADR，现有模块评审者按
[CODEOWNERS](../../.github/CODEOWNERS) 与 [开发工作流](../development-workflow.md)
审核。部署责任人负责环境选择、备份保管及现场验收；这里不新增个人授权或所有者。
当前采用用户明确的单维护者模式，上述职责可以由同一人承担。自动化检查和独立代理
复审辅助维护，但不构成人员冗余；新增第二维护者不是当前交付的前置条件。
行为改变时说明替代决策与迁移条件，旧决定标记 Superseded 并保留替代链接；
尚未实现的方案标记 Proposed，不能只因文档获批就宣称运行时支持。

每次维护至少复核调用入口、实际失败分支和一个相关测试；修改协议或数据语义时同时
更新兼容/回滚条款。源码链接指向同一 checkout，测试名称用于定位，不代表本次重新运行。
相关发布记录见 [CHANGELOG](../../CHANGELOG.md)。

Issue #235 的本次交付、验证与剩余验收见
[执行记录](../issue-triage/issue-235-adr-decision.md)。
