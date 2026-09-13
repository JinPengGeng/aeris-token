# GitHub 交接与恢复入口

本 fork 的跨电脑交接以 [GitHub 仓库](https://github.com/JinPengGeng/aeris-token)、
[开放 PR](https://github.com/JinPengGeng/aeris-token/pulls)、
[Project #1](https://github.com/users/JinPengGeng/projects/1) 和本文件为准。
[维护交接记录](https://github.com/JinPengGeng/aeris-token/issues/235#issuecomment-5655072498)
串联后续检查点。聊天摘要和本机临时目录不作为唯一恢复来源。

## 当前检查点

### 当前现场 — 北京时间 2026-09-14 07:14

以下内容覆盖并取代本文件下方的 02:29 历史快照，恢复时以本节和 GitHub 实时状态为准。

- `origin/main`：`f502c3a5144724a0c3c62271c5ab21da27b1503c`，已包含 #401、#402、#407、#415、#416、#418、#420、#423、#424、#425、#426 及交接刷新。上述 PR 的四项 required checks 均已成功并以 squash 合并；父 #205/#214/#215 保持开放。
- 当前开放 PR：无。#425 head `1409d5ed1e6944a2b2afbfb806ee19e68ab9db3a`、#426 head `c9faf377d85d2219682ce22e5d182895d664c905` 均通过 Rust CI、Frontend CI、Automation Policy、Dependency Audit 四项 required checks 后自动 squash 合并。
- #419 已完成 P1/Core/High/M/Accepted 切片：`AETHER_GATEWAY_CONTROL_CONTEXT_TIMEOUT_MS`（默认 30000ms，正值 clamp 1..=120000）只覆盖 `ai_public` 控制上下文解析；真实 PostgreSQL 暂停 3 次 fresh key 502（约 1.004–1.006 秒），恢复 3 次 401，暂停期间 local permits 8/0/8、liveness 200。候选与最终 hosted 结果已写入 PR #420、Issue #419 及主干 `docs/issue-triage/issue-419-*`。
- 实时开放 Issue 清点：44 个已标优先级的 Issue（21 个 P1、23 个 P2），父子需求重叠，数量不等于独立开发包。#254、#300、#303、#307、#214、#215、#220、#223、#224、#225、#235、#255 等父项仍需各自剩余验收。
- 本轮没有写 upstream。主工作树 `/Users/fengying/workspace/aeris-token` 的 3 个已跟踪修改和多个未追踪文档/fixture 是用户现场，必须保留；交接文档已完整上传 fork，恢复以 `origin/main` 和对应 Issue/PR 记录为准。

恢复顺序：先刷新 `origin/main`、开放 PR/Issue 和 Project #1；当前没有待合并 PR，继续工作时从父项和剩余 P1 队列选择下一项，重新建立独立 Issue/PR/评审/门禁证据链。#419 的实现、验证和边界已归档，不要重复开发。

核验时间：北京时间 2026-09-14 07:14。已停止启动新的实现，当前只保留现场整理、验证和同步记录。

### 历史收尾快照 — 2026-09-14（已被上方当前现场覆盖）

当前 `origin/main` 为 `174afcc0ab9b1a91a7c305f1f806ffe12d5feeb1`，开放 PR 为 0。
开放 Issue 为 44（P1 21、P2 23；`status:in-progress` 17、`status:triage` 17、
`status:blocked` 10）。Project #1 仍以 Issue 生命周期标签为状态来源；合并子切片
不自动关闭父项。

已合并代表交付：#401 Redis TIME 租约、#402 安全配置迁移、#407 流终止前持有
target 许可、#415 缺凭据 401 契约、#416 key-scoped RPM reset、#418 stream
observer 借用 chunk、#420 公共控制上下文总期限，以及 #423 交接刷新；均通过四项
required checks 后进入 main。父 #205/#214/#215/#254 等剩余范围仍开放。

下一步继续处理 #214、#215、#220、#223、#225、#255、#300/#206、#307/#217 的
剩余验收（故障契约、日志/SSE、供应链、备份恢复、文档一致性、mutation audit、
Gateway 资金生命周期和生产告警）。恢复仅需 `git fetch origin --prune`、读取 main、
开放 PR/Issue 与 Project；本轮未修改 upstream、保护规则、生产数据或用户主工作树。

- 已核验主干：`d071e55068fed700f45fa91cedccf809226e010c`，最近合并 PR #397。
- 开放需求库存：47 个 Issue，23 P1 / 24 P2；父子范围重叠，不能当作 47 个独立开发包。
- 开放 PR 为 #400、#401、#402、#404；#401 已启用受保护 squash auto-merge，#402/#404 保持 Draft。
- 本文档通过 [PR #400](https://github.com/JinPengGeng/aeris-token/pull/400) 交付；后续状态以 GitHub 当前事件为准。
- #396 审计落盘失败指标和告警已合并为 `01caaf5ce6606c4955b8c1ee4816bce8265b1d7b`。
- #397 请求取证读取与敏感读取审计已合并为上述主干 SHA。#255 仍开放，当前授权状态不等于历史认证证据。
- 单维护者模式已接受；不要求第二维护者，继续执行四项 required checks、独立评审和受保护 squash merge。

## 从另一台电脑恢复

新目录克隆即可取得已合并代码和交接文件；如果复用已有克隆，先检查工作区，保留自己的改动。

```sh
git clone https://github.com/JinPengGeng/aeris-token.git
cd aeris-token
git fetch origin --prune
git log -1 --format='%H %s' origin/main
gh pr list --repo JinPengGeng/aeris-token --state open --limit 100
gh issue list --repo JinPengGeng/aeris-token --state open --limit 100
```

先读本文件、[交付 TODO](delivery-todo.md) 和 [开发工作流](../development-workflow.md)，
再读目标 Issue 最新评论与 PR 描述。接手未合并 PR 时，在干净目录执行
`gh pr checkout <PR编号> --repo JinPengGeng/aeris-token`，核对 `git rev-parse HEAD`
与 PR 的 `headRefOid` 一致，再按其验证记录复现。若分支落后，使用新主干重新集成并刷新验收。
不要仅凭旧评论的通过状态合并新的 head。

本机 macOS 使用 `rtk` 包装 shell 命令；新电脑没有安装时，上述原生命令可直接执行。
运行依赖、数据库/Redis fixture 和具体测试命令以相应 PR 为准，真实凭据由部署环境提供。

## 当前在制范围与决策

| 任务 | 优先级 | GitHub 现场 | 恢复时下一步 |
| --- | --- | --- | --- |
| [#398](https://github.com/JinPengGeng/aeris-token/issues/398) 配置迁移路径安全 | P1 | [Draft #402](https://github.com/JinPengGeng/aeris-token/pull/402)，`fix/issue-398-service-config-safe` | 修订稿 13 个真实文件系统测试及 Rust 1.95 Clippy/fmt 通过；需新 head 独立复审、Linux/root 运行边界确认及 hosted 检查 |
| [#399](https://github.com/JinPengGeng/aeris-token/issues/399) Redis lease 时间源 | P1 | [PR #401](https://github.com/JinPengGeng/aeris-token/pull/401)，`fix/issue-399-redis-server-time` | 本地实现/独立评审已完成，严格 harness 129 passed；Redis 7.4.11/8.10.1 新增 6 项各通过，等待 hosted 四项门禁 |
| [#403](https://github.com/JinPengGeng/aeris-token/issues/403) nightly tunnel 制品 | P2 | [Draft #404](https://github.com/JinPengGeng/aeris-token/pull/404)，`feat/nightly-signed-tunnel-artifacts` | 当前草稿会使缺签名配置的 nightly 失败，禁止合并；先实现全未配置时保留 gateway nightly 的条件分支，再补真实签名/构建/独立评审 |

上述三项实现、决策和验证命令均已推送到 fork，可以直接从对应 PR 恢复。核验的 head SHA：

```text
#401 49b3f35e7603f698a3437bba2a574e7a89fc8fdd
#402 ce7d747be584e82f3b503406c82ab96460c24bbd
#404 f539d2da65ab87469dae7873f253ae69c1667abd
```

#398 第一版独立评审拒绝后已修正祖先 FD walk、共享目录权限策略、FIFO 和真实迁移覆盖，
不能把第一版的拒绝或本机测试通过当作修订稿已完成独立验收。#399 不依赖 #52 的 HalfOpen 新功能。
未完成草稿不混入 `main`；原主工作树的用户既有改动和历史 worktree 全量清点仍留待收尾。

签名配置只读核验：仓库及受保护 `release` Environment 均未列出 tunnel signing secret，
仓库未列出 public trust variables。缺配置不会被当作签名发布验收通过；真实发布仍服从
`release` Environment 审批。私钥或生产数据不写进交接文档。

## 看板核验更正

Project 分页误读曾导致无依据的批量字段覆盖。按更新前快照恢复 68 个字段后，
02:04 已完整读取当时的 389 张卡，核验原有 Status/Priority/Area/Risk/Size/Decision 全部一致。
当时 46 个开放 Issue 在更改前已经各有卡，不能计作“本轮补齐缺失卡”。后续仅根据具体任务事实
定向更新字段，不从状态机械推导 Risk、Size 或 Decision。

## 07:00 后收尾标准

1. 停止启动新实现，等待或停止本轮已有任务，记录实际结果。
2. 已通过独立评审和 required checks 的 PR 按授权合并；其余保留 Draft PR，记录 head SHA、失败原因和下一步。
3. 更新本文件和交付 TODO，列出准确开放项、优先级、阻塞及未部署边界；回读 GitHub 代码、PR、Issue 与 Project。
4. 清点本轮 worktree 的 dirty/untracked/unpushed 状态。用户原有修改保留，不执行清空、强推或批量提交；发现仅本地存在的工作时先审查来源和敏感内容，再保存可恢复分支。
5. 仅停止已识别的本轮测试服务/进程。记录仍保留的本地资源和未上传原因，不把保留本地资源说成清理完成。

生产历史资金影响、真实恢复演练、部署签名配置与生产验收的未知结论继续保留在相应 Issue。
文档、合成测试或 PR 合并都不能替代这些证据。
