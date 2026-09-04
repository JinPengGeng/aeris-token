# GitHub 自动化与 Agent 架构

本文描述当前 v2 终态。现行自动化以仓库中的 workflow、registry 和 policy 为准；历史设计只用于审计，不能按历史章节操作。

## 1. 当前边界

- 所有 `main` 变更通过 PR；普通 PR 使用 squash merge。
- 上游同步唯一入口是 `.github/workflows/sync-upstream-minimal.yml`。它只使用 `GITHUB_TOKEN`，不使用 AI、Writer、Finalizer、attestation 或 checkpoint 状态机。
- 同步进度由 Git DAG 的 `git merge-base` 决定：将 `upstream/main` 真合并到固定的 `sync/upstream` 分支，复用或创建唯一同步 PR。同步 PR 必须使用 **merge commit**，不得 squash 或 rebase；合并后验证 upstream 仍是 `main` 的祖先且合并提交恰有两个 parent。
- 冲突、`.github/**` 漂移或其他异常均 fail closed，并通过幂等告警 issue 暴露；不会自动解决冲突或覆盖 `main`。
- 旧 checkpoint、AI conflict resolver/reviewer、Writer、Publisher、Policy Finalizer 或 persistent native auto-merge 仅在历史归档中保留，不是现行流程。

## 2. Issue triage 当前实现

`.github/workflows/issue-triage.yml` 当前只有两个 job：

1. `prepare`：从 default branch checkout 受信配置，确定性校验事件/actor/开关，为新 Issue 添加 `status:triage`，并预约 managed comment。
2. `analyze`：读取 reservation artifact；仅在确实 reserved 时调用一次只读 AI，否则透传 terminal 状态；随后确定性校验并更新唯一 managed comment。

AI 没有 shell、代码写入或 GitHub 写入凭据。标签和 comment 的写回由确定性代码完成：标签只使用 registry/policy 允许的名称，comment 使用固定 marker 并按 fingerprint 收敛。managed comment 是 best-effort projection，不是锁、租约、CAS 或审计数据库；重复事件和模型失败必须 fail closed/可观察。

实际配置接口只有以下五个 repository 配置名；文档不写入任何秘密值：

- Secret：`AERIS_AI_API_KEY`
- Variables：`AERIS_AGENTS_ENABLED`、`AERIS_AI_BASE_URL`、`AERIS_AI_MODEL`、`AERIS_AI_MODEL_REVIEWER`

模型 registry（`.github/ai-executors.json`）只保留 executor `openai-chat-v1`，并将 `agent_analysis` 路由到它。当前 Agent registry 中的逻辑角色为 `triage`、`planner`、`reviewer`，均为 read-only；reviewer 只读 PR/repository 并更新 managed comment。模型 ID 必须来自受信 registry/Variables，Issue 或评论不能指定任意模型。

## 3. 受保护配置与仓库设置

`.github/agents.yml`、`.github/automation-policy.yml`、workflows 和相关 contract tests 是受信控制面，运行时从 default branch 的精确 SHA 读取。当前仓库设置基线只有 `main-protection` ruleset：`main` 禁止直接推送/删除/强推，要求 PR、讨论解决和以下三个 strict required checks：

- `Rust CI / check`
- `Frontend CI / check`
- `Automation Policy / gate`

Ruleset 和远端 Variables/Secrets 由 GitHub Settings 管理，不能从文档推断秘密值或远端开关状态。`release` Environment 仍需人工审批；只读 AI 不得访问发布凭据。CI 按路径选择性执行，无关 job 以 skipped 作为通过，主干 push/手动 dispatch 全量执行。

## 4. 上游同步运行说明

每日调度（也可 `workflow_dispatch`）在 `sync-upstream-main` concurrency group 中运行且不取消已有运行。workflow 在 default branch 上执行，使用 `GITHUB_TOKEN` 维护 `sync/upstream` 和唯一同步 PR，并显式 dispatch 必要 CI。干净且没有 `.github/**` 漂移时才允许启用 GitHub 原生 auto-merge，方法固定为 **merge commit**；任何冲突、策略漂移、检查失败、fetch/merge/readback 异常都停止并创建幂等告警 issue。

同步完成后必须重新获取 `origin/main`，确认：

```text
git rev-list --count origin/main..upstream/main == 0
```

并确认新提交有两个 parent。同步 PR 是唯一例外：它使用 merge commit 保持 upstream/main 与 fork main 的祖先连通；所有普通 PR 仍使用 squash。同步没有 checkpoint 文件，也没有人工或 AI 冲突合并流程。

## 5. 实施状态

- **Phase 0：已完成**——策略契约、威胁模型、kill switch、仓库设置和 contract tests。
- **Phase 1：已完成**——minimal sync 的 Git DAG/merge-base 闭环、唯一 PR、merge commit 纪律和 fail-closed 告警。
- **Phase 2：已完成**——Actions-only 只读 issue triage（prepare/analyze）、受限 AI 调用、确定性标签和 managed comment。
- **Phase 3 及以后：不存在/暂不实施**——不实施 Candidate/Writer/Publisher、AI conflict resolver/reviewer、Finalizer、autonomous code write 或自动合并生产链。任何未来改变必须先更新本文和对应 contract tests；不要依据历史归档部署。

## 6. 验证与审计

自动化 contract 由 `Frontend CI / check` 中的 `Automation Contracts` job 执行，覆盖 Agent workflow、AI executor、同步告警和 bounded fetch。文档或 workflow 变更至少运行该 job 对应的 contract tests，并执行 `git diff --check`。研究报告保留历史背景，不是当前运维规范。