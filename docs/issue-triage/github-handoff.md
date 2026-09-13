# GitHub 交接与恢复入口

本 fork 的跨电脑交接以 [GitHub 仓库](https://github.com/JinPengGeng/aeris-token)、
[开放 PR](https://github.com/JinPengGeng/aeris-token/pulls)、
[Project #1](https://github.com/users/JinPengGeng/projects/1) 和本文件为准。
[维护交接记录](https://github.com/JinPengGeng/aeris-token/issues/235#issuecomment-5655072498)
串联后续检查点。聊天摘要和本机临时目录不作为唯一恢复来源。

## 当前检查点

核验时间：北京时间 2026-09-14 02:04。计划持续开发至当日 07:00，之后停止启动新实现，
整理、验证和同步现场；本检查点不是 07:00 的最终收尾结果。

- 已核验主干：`d071e55068fed700f45fa91cedccf809226e010c`，最近合并 PR #397。
- 开放需求库存：46 个 Issue，23 P1 / 23 P2；父子范围重叠，不能当作 46 个独立开发包。
- 本文档 PR 创建前开放 PR 为 0；后续状态以 GitHub 当前事件为准。
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

| 任务 | 优先级 | 当前结论 | 恢复时下一步 |
| --- | --- | --- | --- |
| [#398](https://github.com/JinPengGeng/aeris-token/issues/398) 配置迁移路径安全 | P1 | 草稿独立评审未通过：祖先路径竞态、共享父目录权限修改、真实迁移测试缺口 | 先修正并验证安全边界；通过独立复审后才进入受保护合并 |
| [#399](https://github.com/JinPengGeng/aeris-token/issues/399) Redis lease 时间源 | P1 | 独立实施 acquire/renew/live-count 在 Lua 内使用 Redis TIME | 真实 Redis 并发/过期/续租/ACL 故障验证；不依赖 #52 HalfOpen 新功能 |
| [#205](https://github.com/JinPengGeng/aeris-token/issues/205) nightly tunnel 制品残项 | P2 | 工作流草稿尚未验收；签名文件命名、信任输入和发布边界需要修正 | 签名/验签与负路径实测，记录未配置和未发布状态后提交独立 PR |

上述在制草稿在本检查点尚未推送，不能声称已可跨电脑恢复。其实现、验证和审查结论须在收尾前
保存到本 fork 分支/Draft PR；后续更新本表为实际 PR、head SHA 和可执行下一步。
不要将未完成草稿混入 `main`。

签名配置只读核验：仓库及受保护 `release` Environment 均未列出 tunnel signing secret，
仓库未列出 public trust variables。缺配置不会被当作签名发布验收通过；真实发布仍服从
`release` Environment 审批。私钥或生产数据不写进交接文档。

## 看板核验更正

Project 分页误读曾导致无依据的批量字段覆盖。按更新前快照恢复 68 个字段后，
已完整读取 389 张卡，核验原有 Status/Priority/Area/Risk/Size/Decision 全部一致。
46 个开放 Issue 在更改前已经各有卡，不能计作“本轮补齐缺失卡”。后续仅根据具体任务事实
定向更新字段，不从状态机械推导 Risk、Size 或 Decision。

## 07:00 后收尾标准

1. 停止启动新实现，等待或停止本轮已有任务，记录实际结果。
2. 已通过独立评审和 required checks 的 PR 按授权合并；其余保留 Draft PR，记录 head SHA、失败原因和下一步。
3. 更新本文件和交付 TODO，列出准确开放项、优先级、阻塞及未部署边界；回读 GitHub 代码、PR、Issue 与 Project。
4. 清点本轮 worktree 的 dirty/untracked/unpushed 状态。用户原有修改保留，不执行清空、强推或批量提交；发现仅本地存在的工作时先审查来源和敏感内容，再保存可恢复分支。
5. 仅停止已识别的本轮测试服务/进程。记录仍保留的本地资源和未上传原因，不把保留本地资源说成清理完成。

生产历史资金影响、真实恢复演练、部署签名配置与生产验收的未知结论继续保留在相应 Issue。
文档、合成测试或 PR 合并都不能替代这些证据。
