# Issue #235：ADR 文档切片交付记录

日期：2026-09-13。范围：已接受的 P2 ADR index、恢复原子性、隧道版本及 usage retry/DLQ。
来源：[Issue #235](https://github.com/JinPengGeng/aeris-token/issues/235) 及维护者分诊评论。

## 实施与判断

从实时 `origin/main` 的 `4a74b11ef9a4756f897a33f1ce0b18475a2f040d` 创建隔离分支，
核验 origin 为 fork `JinPengGeng/aeris-token`，保留用户主树和并行资金 worktree。
使用 [ADR 索引](../adr/README.md) 及三篇描述性文件名，不补造缺失的历史编号。
CONTRIBUTING 增加入口与部分交付使用 `Ref` 的规则；无运行时、依赖、工作流或
CODEOWNERS 变更。

三项影响结论的源码复核已反映到 ADR：恢复使用检查点补偿而非全库事务；隧道 helper
虽保留 v1 fallback/clamp，当前 `ws_proxy` 已强制显式 `1..=3`；usage-core 存在纯合同，
但 runtime 并未依赖它，不能把声明类型当作生产接线。每篇以 Accepted 记录当前有限
合同，同时保留失败边界、替代方案、兼容/回滚与现有角色责任。

## 验证

逐一读取 ADR 引用的关键入口、失败分支和测试名称；6 个改动文件中的 65 个 Markdown
相对链接目标均存在，没有待验证的 fragment anchor；26 个测试/函数名核对及 `git diff --check`
通过。本次为文档切片，不运行 Cargo、不占用并行数据库，
也不把历史演练记录写成本次实测结果。远端 PR checks 与独立评审由交付流程继续核验。

## 剩余验收

| #235 项目 | 本次结果 |
| --- | --- |
| ADR 索引与三篇“为什么”决策 | 已记录当前代码、测试、取舍、兼容/回滚与维护角色 |
| CHANGELOG | 主干已有 [CHANGELOG](../../CHANGELOG.md)，本切片未重新宣称新增 |
| 第二维护者与 bus factor | 未完成；需要真实参与者同意、权限及评审实践，不能仅添加名字 |
| 迁移编号政策与 wreq 选型/升级规则 | 未在本切片实施，需要单独源码复核与决策 |
| churn/复杂度热点、dead code 与半成品接线 | 未在本切片实施；旧行数/churn 快照不能替代当前缺陷确认 |
| 完整结算幂等链或 usage-core 接线 | 不因本文完成；需按实际运行链路独立验收 |
| 生产恢复、历史 tunnel 二进制矩阵、DLQ marker/容量 | 各篇明确保留，不以文档代替演练 |

因此 PR 使用 `Ref #235`，保持父 Issue 开放。本文不修改共享 TODO/Project 文件，
合并与状态同步由主线程统一协调。
