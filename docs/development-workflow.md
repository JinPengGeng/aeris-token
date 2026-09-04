# GitHub 开发工作流

本文定义 `aeris-token` 从需求进入、开发、审查到发布的默认协作流程。所有对 `main` 的变更都应通过 Pull Request (PR) 合并。

## 1. 一次性仓库配置

仓库管理员应在 GitHub 完成下列配置；这些设置无法由仓库文件强制生效。

### Labels

先创建以下标签，Issue Form 和自动路径标签依赖其名称完全一致：

| 类别 | 标签 |
| --- | --- |
| 类型 | `type:bug`、`type:feature`、`type:architecture` |
| 领域 | `area:scheduler`、`area:pool`、`area:protocol`、`area:frontend`、`area:tunnel`、`area:ci`、`area:docs` |
| 优先级 | `priority:P0`、`priority:P1`、`priority:P2` |
| 状态 | `status:triage`、`status:ready`、`status:in-progress`、`status:blocked` |
| Agent 控制 | `agent-analyze`（允许分析外部 Issue）、`agent-ready`（允许代码实施） |

`.github/workflows/labeler.yml` 读取 `.github/labeler.yml`，并为同仓库分支 PR 的改动路径自动添加 `area:*` 标签。外部 fork PR 不授予写 token，因此不会自动写标签，需要维护者人工分类。路径标签是辅助分类，代码作者仍需确认其准确性。

### `main` 分支保护

在 `Settings -> Branches` 为 `main` 配置 Branch protection rule；也可以用等价的 Ruleset：

1. 禁止直接推送、强制推送和删除分支。
2. 要求通过 PR 合并并解决全部 review 讨论。当前仓库只有一名维护者时将审批数设为 0；增加协作者后再启用至少一位审批和 CODEOWNERS review。
3. 要求状态检查 `Rust CI / check`、`Frontend CI / check` 和 `Automation Policy / gate` 成功后才可合并。该设置由 GitHub 远端治理执行；`main` 当前使用 `main-protection` ruleset。

   两个 CI 工作流在 PR 上按变更路径选择性执行 job：无相关改动时对应 job 以 `skipped` 结论跳过并视为通过门禁，聚合 check 只将 `failure` 或 `cancelled` 判为失败。推送到 `main` 与手动 `workflow_dispatch` 始终全量执行，作为主干完整性的兜底。
4. 只允许 Squash merge并在合并后自动删除源分支。增加独立 reviewer 后启用 CODEOWNERS review。

仓库管理员应保留紧急恢复能力，仅用于仓库解锁，并在后续 Issue 中记录原因。

### Project 与发布环境

建立 GitHub Project v2，字段为 `Status`、`Priority`、`Area`、`Risk` 和 `Target release`。使用 Project 内置 workflow：新 Issue 进入 `Inbox`，PR 创建后进入 `In progress`，合并 PR 后进入 `Done`。

为发布创建受保护且需要人工审批的 `release` Environment。发布凭据只放入该 Environment；Issue 或外部 PR 的文本不得在可访问 secrets 的工作流中执行。不要以 `pull_request_target` checkout 外部 PR 代码。

当前只读 AI 使用 `agent` Environment（不得包含写入或 release secret）；`release` Environment 保留人工审批。历史 Writer Environment/凭据设计已归档，不要创建或配置。

### Fork 上游同步

唯一入口是 `.github/workflows/sync-upstream-minimal.yml`，每日运行或手动 dispatch，使用 `GITHUB_TOKEN` 和 `sync-upstream-main` concurrency group。它不读取 checkpoint/state 文件，不使用 AI、Writer 或 Finalizer；以 `git merge-base`/真实 Git DAG 判断进度，将 `upstream/main` 真合并到固定 `sync/upstream` 分支，并复用或创建唯一同步 PR。

同步 PR 是普通 PR 的唯一合并例外：必须使用 **merge commit**，禁止 squash/rebase，以保持 upstream/main 与 fork main 的祖先连通。干净且无 `.github/**` 漂移时才启用 native auto-merge（merge commit）；合并后检查 `git rev-list --count origin/main..upstream/main == 0` 且提交恰有两个 parent。冲突、漂移、检查失败或任何异常均 fail closed，并创建幂等告警 issue；不会自动解决冲突或写入 `main`。普通开发 PR 仍一律 squash。

## 2. Issue 到 PR

1. 从 Issue Form 创建 Bug、Feature、Scheduler/Failover 或 Config 变更。提交后均进入 `status:triage`。
2. 维护者补齐优先级、领域、验收标准和目标版本。可执行开发的 Issue 标为 `status:ready`。
3. 只读 Agent 只能提供分析和 managed comment；它不能创建分支、写代码、修改 Ruleset 或读取发布 secrets。
4. 从 Issue 创建短生命周期分支，PR 描述使用 `Closes #<issue-number>`。PR 模板中的风险、回滚和验证项必须完成。
5. CI、CODEOWNERS 审查和所有讨论通过后，以 Squash merge 合入。GitHub 自动关闭被 `Closes` 引用的 Issue，Project 将其移到 `Done`。

Agent 的模型路由、权限隔离、事件幂等和上游同步门禁见 [GitHub 自动化与 Agent 架构](automation-architecture.md)。该架构默认关闭新 Agent；只有对应阶段的 workflow、测试和仓库设置全部完成后才可启用。

仓库中的只读 Agent registry 与开关应分别审计；远端实际状态以 GitHub Variables、Actions run 和 managed comment 现场证据为准。当前不启用 Candidate、Publisher、Writer 或 Finalizer。

Scheduler、重试、路由、池、额度或故障转移变更必须说明状态转换、确定性选择规则、依赖失败行为和回滚，并覆盖失败与恢复测试。

## 3. 本地验证

按改动范围执行必要检查：

```powershell
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

Set-Location frontend
npm ci
npm run lint:check
npm run type-check
npm run test:run
npm run build
```

不要在只读 CI 中使用 `npm run lint`，因为该脚本带 `--fix`。CI 应调用一个不修改工作区的 lint 命令，例如后续增加的 `npm run lint:check`。

## 4. 依赖、安全与发布

Dependabot 每周为 Cargo、`frontend` npm 和 GitHub Actions 创建更新 PR。依赖更新与其他 PR 一样需要 CI 和审查；重大版本升级需要单独记录兼容性与回滚风险。

发布只能由已通过 `main` CI 的 tag 触发，并须经 `release` Environment 人工审批；`workflow_dispatch` 仅用于不发布的手工构建验证。发布工作流的镜像、下载链接和 Release 目标必须使用当前仓库所有者，不能保留上游 fork 的 `fawney19/Aether` 标识。具有写权限或可访问发布 secrets 的第三方 GitHub Actions 应优先固定到完整 commit SHA；其余 Action 至少固定到明确版本，并由 Dependabot 持续更新。