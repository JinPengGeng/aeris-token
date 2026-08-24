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
3. 要求状态检查 `Rust CI / check` 和 `Frontend CI / check` 成功后才可合并。`Automation Policy / gate` 只有在单 Writer PoC 完成、来源绑定验证完毕后才加入 required checks；在此之前不得把它标记为已启用。
4. 只允许 Squash merge并在合并后自动删除源分支。增加独立 reviewer 后启用 CODEOWNERS review。

仓库管理员应保留紧急恢复能力，仅用于仓库解锁，并在后续 Issue 中记录原因。

### Project 与发布环境

建立 GitHub Project v2，字段为 `Status`、`Priority`、`Area`、`Risk` 和 `Target release`。使用 Project 内置 workflow：新 Issue 进入 `Inbox`，PR 创建后进入 `In progress`，合并 PR 后进入 `Done`。

为发布创建受保护且需要人工审批的 `release` Environment。发布凭据只放入该 Environment；Issue 或外部 PR 的文本不得在可访问 secrets 的工作流中执行。不要以 `pull_request_target` checkout 外部 PR 代码。

单 Writer 上线时另建两个 Environment：`agent` 不设人工审批，只暴露模型凭据，绝不包含 GitHub 写凭据、Writer App 私钥或 release secret；`writer` 保存 Writer App ID/私钥，并用于 Publisher、Finalizer 和同步。这里是管理员待完成的远端设置清单，仓库文件本身不能证明已经配置。

### Fork 上游同步

`.github/workflows/sync-upstream.yml` 每天中国时间 05:00 检查 `fawney19/Aether` 的默认分支；发现上游新提交时，从受保护 `main@SHA` 的 `.github/upstream-sync-state.json` 读取上次已合并 checkpoint，只计算 checkpoint 之后的上游增量。工作流始终复用固定的 `automation/sync-upstream` 分支和唯一的开放同步 PR，不会因每日运行而重复创建 PR。维护者手工关闭该 PR 后，定时同步会持续暂停；只有手工运行工作流并设置 `resume=true` 才会尝试重新打开原 PR，无法重新打开时仅允许该次显式运行创建一个替代 PR。

同步不会绕过 PR 直接写入 `main`。`.github/upstream-sync-policy.yml` 中的 fork-owned 路径在三方合并前被过滤并保留 fork 版本；auth/migrations/security 与未知路径发布为人工审查 PR，`.gitmodules`、`.pem`、`.key`、`.p12` 等敏感文件 fail closed。`Automation Policy / gate` 对同步只证明 required-check health，不是 eligibility attestation；只有 trusted prepare 输出为 `eligible` 或经过冲突 final attestation 的 `conflict_ai_review`、精确 commit trailer 和 merge helper 再次校验全部通过时才允许 direct merge。唯一的冲突例外是逐个满足 UTF-8、普通文件 mode `100644` 和 modify/modify 的上游文本冲突：无 GitHub 写凭据的 AI Resolver 生成实际 candidate artifact，不同 model ID 的独立 AI Reviewer 审查；trusted deterministic verifier 重新物化结果并将 artifact 链、当前 head/tree、base、checkpoint、upstream SHA 与 policy 精确绑定到 final attestation。干净生成的 managed 同步 PR 只会在精确 head/base 的 required checks 成功，且 merge helper 重新证明 branch protection 为 strict、admin-enforced、零 bypass、无 active branch ruleset 后，使用一次 `PUT /repos/{owner}/{repo}/pulls/{number}/merge`、`merge_method=squash` 和该 SHA 执行 server-side direct squash；不会启用 GitHub native auto-merge。无论 mutation 的响应成功、失败或不确定，工作流都只独立回读一次，且必须证明同一 PR 由 Writer App bot 合并、head/base 一致、`auto_merge=null`，以及 merge commit 是以 base 为唯一 parent 的 squash commit，才记为成功。检查未成功或超时、治理证明漂移、mutation/readback 失败，以及其他冲突、二进制、编码或 mode 不符、Reviewer model-ID 不独立、artifact/attestation 或任一绑定漂移、上游历史重写、非法 state/policy 和无法识别的同步分支 tip 均 fail closed，不会推进 checkpoint 或覆盖远端分支；固定分支和开放 PR 会被保留，供后续同步复用。重建固定分支前仍会 disarm 历史遗留的 native auto-merge，避免旧授权残留。人工解决这些冲突时，应使用普通维护者 PR 同时提交解决结果和新的 `last_integrated_sha`。

## 2. Issue 到 PR

1. 从 Issue Form 创建 Bug、Feature、Scheduler/Failover 或 Config 变更。提交后均进入 `status:triage`。
2. 维护者补齐优先级、领域、验收标准和目标版本。可执行开发的 Issue 标为 `status:ready`。
3. 只有维护者添加 `agent-ready` 后，AI Agent 才能创建分支和 Draft PR。Agent 不得直接写入 `main`、修改 Ruleset 或读取发布 secrets。
4. 从 Issue 创建短生命周期分支，PR 描述使用 `Closes #<issue-number>`。PR 模板中的风险、回滚和验证项必须完成。
5. CI、CODEOWNERS 审查和所有讨论通过后，以 Squash merge 合入。GitHub 自动关闭被 `Closes` 引用的 Issue，Project 将其移到 `Done`。

Agent 的模型路由、权限隔离、事件幂等、上游同步 checkpoint 和自动合并门禁见 [GitHub 自动化与 Agent 架构](automation-architecture.md)。该架构默认关闭新 Agent；只有对应阶段的 workflow、测试和仓库设置全部完成后才可启用。

仓库中的只读 Agent registry 与开关应分别审计；远端实际启用状态必须以 GitHub Variables、Actions run 和 managed comment 现场证据为准。启用只读阶段不等于授权 Candidate、Publisher、Policy required check 或 Finalizer。单 Writer production flags 和 required Policy 仅在全部 PoC、撤销演练和稳定观察后启用。

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
