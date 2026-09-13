# GitHub 交接与恢复入口

本 fork 的跨电脑交接以 [GitHub 仓库](https://github.com/JinPengGeng/aeris-token)、
[开放 PR](https://github.com/JinPengGeng/aeris-token/pulls)、
[Project #1](https://github.com/users/JinPengGeng/projects/1) 和本文件为准。
[维护交接记录](https://github.com/JinPengGeng/aeris-token/issues/235#issuecomment-5655072498)
串联后续检查点。聊天摘要和本机临时目录不作为唯一恢复来源。

## 当前检查点

### 最新现场 — 北京时间 2026-09-14 06:38

以下内容覆盖并取代本文件下方的 02:29 历史快照，恢复时以本节和 GitHub 实时状态为准。

- `origin/main`：`551d9a8922622d6a72d9e865cc7efb95ae83509b`，已包含 #415（公开缺凭据 401）、#416（RPM reset 查询）、#418（标准流观测借用 chunk）及本次交接文档刷新 PR #421。#416/#418/#421 的四项 required checks 均已成功，父 #214/#215 保持开放。
- 当前开放 PR 只有 [#420](https://github.com/JinPengGeng/aeris-token/pull/420)，已同步主干后的 head `016b567fa9d423e53a3aeb23797666e388e1fbcc`，已启用受保护 squash auto-merge，等待四项 required checks；不要沿用本地候选 binary 或旧 run 代替最终 head 验收。
- #420 对应 [#419](https://github.com/JinPengGeng/aeris-token/issues/419)，P1/Core/High/M/Accepted，Project 为 In review；父 #214 仍开放。新增 `AETHER_GATEWAY_CONTROL_CONTEXT_TIMEOUT_MS`（默认 30000ms，正值 clamp 1..=120000）只覆盖 `ai_public` 控制上下文解析。
- #419 候选真实演练已经证明：PostgreSQL 暂停时三个 fresh key 为 502（约 1.004–1.006 秒），恢复后三个 fresh key 为 401，暂停期间 local permits 为 8/0/8、liveness 为 200；旧行为 binary 同一脚本触及 curl 2 秒期限并得到 000。候选证据不等于最终 head CI，生产部署仍未验收。PR #420 已通过主干变更同步，需针对 `016b567f` 的 checks 重新确认。
- 实时开放 Issue 清点：45 个已标优先级的 Issue（22 个 P1、23 个 P2），父子需求重叠，数量不等于独立开发包。#254、#300、#303、#307、#214、#215、#220、#223、#224、#225、#235、#255 等父项仍需各自剩余验收。
- 本轮没有写 upstream。主工作树 `/Users/fengying/workspace/aeris-token` 的 3 个已跟踪修改和多个未追踪文档/fixture 是用户现场，必须保留；#419 工作树另有作者追加的测试 fixture 已提交为 `e0b00f51a`，恢复以 PR #420 head 为准。

恢复顺序：先刷新 `origin/main`、开放 PR/Issue 和 Project #1；检查 #420 的最终 head 与四项 required checks，满足门禁后自动 squash 合并。若 07:00 前未完成，保留 PR #420 和本节记录，不强行合并；随后从该 PR 继续独立评审、最终 hosted/真实演练和父项更新。

核验时间：北京时间 2026-09-14 02:29。计划持续开发至当日 07:00，之后停止启动新实现，
整理、验证和同步现场；本检查点不是 07:00 的最终收尾结果。

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
