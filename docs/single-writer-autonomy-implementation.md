# 单 Writer App 自主开发实施规格

> 状态：`implementation-in-progress / production-disabled`
> 日期：2026-08-20
> 目标仓库：`JinPengGeng/aeris-token`
> 设计依据：`docs/research/github-autonomous-automation-architecture-research-2026-08-20.md`

## 1. 目标与完成定义

本方案在 GitHub 内运行，不依赖本地电脑常驻。AI 负责生成和测试候选变更；一个最小 Writer App 负责受控分支和 Pull Request 写入；GitHub Actions、分支保护和 native auto-merge 负责确定性门禁与最终合并。

只有以下证据全部成立，才可声明实现完成：

1. Agent job 不持有 GitHub 写凭据、Writer App 私钥或 release secret。
2. Publisher 不执行候选代码，只验证并应用有界 patch。
3. Writer App 仅安装到本仓库，仅有 `contents:write` 和 `pull_requests:write`。
4. PR CI 使用 `pull_request`、只读 token、无 secrets。
5. `Automation Policy / gate` 来自 GitHub Actions，并由 `main` 的受信策略代码计算。
6. `main` 严格要求 Rust、Frontend 和 Policy 三个检查，且来源绑定 GitHub Actions。
7. Finalizer 不直接 merge，只在精确状态复核后请求 native squash auto-merge。
8. 初始无人合并 allowlist 仅覆盖 `docs/automation-canary/**/*.md`。
9. 上游同步复用同一个 Writer App、固定分支和单一开放 PR。
10. 冲突、状态漂移、敏感路径、读取不完整和凭据异常全部 fail closed。
11. `release` Environment 仍需人工审批且管理员不可绕过。
12. 六项 GitHub 现场 PoC 和撤销演练均有 run、PR、SHA 和 API 快照证据。

## 2. 非目标

- 不恢复 Writer、Policy、Merger、Sync 四 App 方案。
- 不建设自有 Merger、merge queue、外部数据库、Temporal 或常驻控制面。
- 不让模型输出、模型置信度、评论或 label 单独授权合并。
- 不允许 Agent 修改 `.github/**`、`CODEOWNERS`、release 或治理配置。
- 不让无人值守流程发布 release、部署生产或读取生产数据。

## 3. 运行拓扑

```text
workflow_dispatch / 受信调度器
  -> Preflight（main 上的受信代码；只读 GitHub）
  -> Codex Agent（临时 runner；模型凭据；无 GitHub 写 token）
  -> candidate.patch + candidate-manifest.json（不可信 artifact）
  -> Publisher pre-token verifier（main 上的受信代码）
  -> 短期 Writer App installation token
  -> agent/issue-<number> + Draft PR
  -> pull_request CI（无 secrets）
  -> Automation Policy / gate（base 受信代码；不执行 PR 内容）
  -> Finalizer（API-only；不 checkout PR head）
  -> GitHub native auto-merge
  -> protected main
```

上游同步走独立确定性入口，但复用 Writer App：

```text
schedule / workflow_dispatch
  -> checkpoint 三方合并
  -> automation/sync-upstream
  -> 单一 managed PR
  -> Rust + Frontend + Policy
  -> native auto-merge
```

## 4. 身份与权限

| 阶段 | 身份 | GitHub 权限 | Secret | 禁止事项 |
|---|---|---|---|---|
| Preflight | `GITHUB_TOKEN` | contents/issues/PR read | 无 | 写入、执行候选代码 |
| Codex Agent | 无 GitHub 写凭据 | `contents:read`（只读 token） | `agent` Environment 的模型 key；无人工审批 | GitHub 写入、Writer/release secret |
| Publisher verify | `GITHUB_TOKEN` | `contents:read`, issues/PR read | 无 | 执行 artifact 或候选脚本 |
| Publisher write | Writer App token | `contents:write`, `pull_requests:write` | App private key | checks/statuses/actions/issues/admin/release |
| PR CI | `GITHUB_TOKEN` | `contents:read` | 无 | 特权 environment、持久 runner |
| Policy | `GITHUB_TOKEN` | contents/PR read | 无 | 模型、候选 checkout、发布自定义 Check API |
| Finalizer | Writer App token | PR/contents write | App private key | checkout PR、直接 merge、admin bypass |
| Release | 维护者 | 现有 release 权限 | `release` Environment；人工审批 | 无人工审批发布 |

Writer App 的残余能力必须明确记录：GitHub 的 `contents:write` 不能硬限制为分支前缀，`pull_requests:write` 也包含广于“创建 PR”的 API 能力。当前硬边界由 `main` 保护、无 bypass、Environment、受信 Publisher 和实时复核共同形成；不得把软件检查描述为 App capability 隔离。

## 5. Candidate artifact contract

Artifact 固定包含两个普通文件：

- `candidate.patch`
- `candidate-manifest.json`

Manifest 使用精确字段集合：

```json
{
  "schema_version": 1,
  "repository": "JinPengGeng/aeris-token",
  "repository_id": 1316750512,
  "task_id": "issue:123",
  "issue_number": 123,
  "base_ref": "refs/heads/main",
  "base_sha": "40-hex",
  "trigger_run_id": "decimal",
  "trigger_run_attempt": 1,
  "patch_sha256": "64-hex",
  "patch_bytes": 1234,
  "created_at": "RFC3339 UTC"
}
```

限制：

- patch 最大 1 MiB、最多 100 个文件、单文件最多 256 KiB 文本变化。
- 拒绝绝对路径、`..`、NUL、反斜杠路径、重复路径、case-fold 冲突。
- 拒绝 symlink、submodule、可执行位变化和非普通文件模式。
- 拒绝 `.github/**`、`CODEOWNERS`、`.gitmodules` 和治理文件。
- patch digest、repo ID、repo 名、task、base、run ID 和 run attempt 必须精确匹配现场。
- 在生成 Writer token 之前完成 schema、digest、路径、mode 和 `git apply --check`。
- Publisher 应用 patch 后再次比较实际 index 路径与验证结果，不信任 manifest 自报路径。

## 6. Agent 执行契约

首个执行器采用固定 SHA 的 `openai/codex-action`：

- Action commit：`52fe01ec70a42f454c9d2ebd47598f9fd6893d56`（`v1.11`）。
- Codex CLI 和 Responses proxy：`0.148.0`。
- Runner：GitHub-hosted Ubuntu。
- checkout：精确 base SHA、`persist-credentials:false`。
- permission profile：`:workspace`；Codex `0.148.0` 的内置 profile 仅允许工作区/临时目录写入，并将命令网络设为 restricted。模型请求由 Action 的本地 Responses proxy 单独转发，不构成候选命令的网络权限。
- safety strategy：`drop-sudo`。
- 模型 endpoint：`${AERIS_AI_BASE_URL}/responses`，必须先做兼容性 canary。
- Codex 是唯一可执行候选内容的步骤；后续只允许运行受信 extractor、校验器和 artifact upload。

受信 extractor 不在 Codex 前复制到 Agent 可写的 runner temp。独立无 Secret 的 `runtime` job 从精确 base SHA 打包不可变 runtime artifact；Codex 结束后，Candidate job 先删除 Agent 可写的同名临时目录，再下载该 artifact，并使用 Codex 前捕获的绝对 Node.js 路径执行。Extractor 以临时 Git directory、临时 index、空 system/global/local config 和原始 object database 的只读 alternate 生成 patch，同时禁用 textconv、external diff、replace refs、fsmonitor、hooks 和继承的 Git 环境变量；不得使用 Agent 可写的 `.git/config`、index、hooks 或 runtime。模型 final message 只作审计说明，不是 Publisher 授权输入。

## 7. Publisher 状态机

```text
disabled -> preflight -> artifact_verified -> token_minted
  -> branch_created|branch_updated -> pr_created|pr_reused -> draft_waiting
```

硬规则：

1. `AERIS_WRITER_ENABLED` 非真值时，Publisher、Finalizer 和 Sync 均在 token mint 前退出；Sync 还要求独立的 `AERIS_UPSTREAM_SYNC_ENABLED` 为真，避免 Writer Draft PoC 意外启动定时同步。
2. 当前 `main` SHA 与 manifest base 不同即 stale，拒绝发布。
3. 分支固定为 `agent/issue-<number>`，每 Issue 最多一个开放 managed PR。
4. 新分支只允许从 manifest base 创建。
5. 已有分支只允许 exact old-SHA `force-with-lease`，且必须关联开放、同仓、同 base、Writer author 的 managed PR。
6. 存在人工关闭的同分支 PR 时默认 tombstone；仅 owner 的显式 `resume_closed` dispatch 可恢复。
7. Publisher 不更新或伪造 CI、review、approval、Policy check。
8. PR 初始为 Draft，body 记录 issue、base、artifact digest 和 run URL。
9. 任一步骤状态不完整、分页截断、GitHub 409/422 或并发漂移均停止，不做猜测性补偿。

## 8. Policy 分类

Policy 输出三个确定性类别：

| 结果 | Check | Finalizer |
|---|---|---|
| `eligible` | success | 可继续实时复核并请求 auto-merge |
| `manual` | success | 保持 Draft，不自动合并 |
| `deny` | failure | 拒绝；需要新 PR 修复 |

初始规则：

- `eligible`：所有变化均为 `docs/automation-canary/**/*.md` 的普通文本新增/修改，且满足数量/大小限制。
- `deny`：控制面、symlink/submodule/mode、来源身份、分支、base 或数据完整性违反硬约束。
- `manual`：其他通过 Publisher 安全边界的普通源码、测试和文档变化。

Policy 不轮询或汇总 Rust/Frontend。GitHub 直接要求三个独立 contexts：

- `Rust CI / check`
- `Frontend CI / check`
- `Automation Policy / gate`

Expected source 只能绑定 GitHub Actions App，不能区分具体 workflow。因此 Policy workflow 必须来自 base 分支，且治理路径不能进入无人合并 allowlist。

## 9. Finalizer 实时复核

Finalizer 在 Policy workflow 完成后运行，但只有以下条件全部成立才行动：

1. 受信 workflow 名称、event、repository ID、base branch 匹配。
2. PR 仍开放，head/base SHA 与触发快照一致。
3. head repo 为本仓库、branch 为 `agent/issue-*` 或受管同步分支。
4. PR author 为配置的 Writer App bot。
5. Policy 对当前 diff 重新计算为 `eligible`。
6. 当前 head 上的 Policy check 为 GitHub Actions expected source 且成功。
7. branch protection 仍是 strict，并包含三个 required checks 及正确 source。
8. 无冲突、无阻塞 review、无未解决讨论、无 manual-only 标签。
9. auto-merge 开关和授权有效期仍有效。

Finalizer 先把 eligible Draft PR 转为 ready，再以 `--auto --squash --match-head-commit` 请求 native auto-merge。任何读取缺失或漂移都不重试写入；下一次可信事件重新计算。

## 10. PoC 顺序

1. **事件语义**：用 disposable PR 分别验证 `GITHUB_TOKEN` 事件抑制和 Writer App `pull_request` CI 触发。
2. **身份与权限**：验证受管分支/PR 成功，`main`、workflow、checks、statuses、admin 和 release 写入失败；记录其他未保护分支的真实残余能力。
3. **Artifact 隔离**：篡改 schema、digest、repo/base/task/path/mode/大小，证明均在 token mint 前失败。
4. **Policy 来源**：将 Policy 加为 required check 并绑定 GitHub Actions；用其他来源同名状态证明不能满足保护。
5. **漂移与 native auto-merge**：逐一构造 head/base/check/draft/conflict/discussion 漂移，证明不会合并。
6. **同步幂等**：连续三轮验证固定分支、最多一个开放 PR和 no-op；冲突、unknown tip、历史重写、人工关闭全部停止。
7. **撤销**：关闭变量、disarm managed PR、suspend App、轮换 key，再验证无新写入。
8. **Release**：触发 release lane，确认仍等待维护者审批且无 Agent secret。

PoC 使用 disposable issue、`docs/automation-canary/` 和 Draft PR；在全部证据通过前，`AERIS_WRITER_ENABLED=false`、`AERIS_UPSTREAM_SYNC_ENABLED=false`、`AERIS_AUTONOMOUS_MERGE_ENABLED=false`，Policy 不加入生产 required checks，Finalizer 不请求 auto-merge。不能因仓库内 workflow 已存在而声称远端 App、Environment、secret、ruleset 或 PoC 已完成。

## 11. 上线与回滚

上线顺序：

1. 合入 disabled 代码和测试。
2. Policy shadow 运行并核对分类。
3. 创建并仅安装一个 Writer App。
4. 配置 `agent` Environment（无人工审批，仅模型凭据）和 `writer` Environment（App ID/私钥），并记录远端设置快照；`release` Environment 继续人工审批。
5. 运行 Draft canary，不启用 auto-merge。
6. PoC 通过后将 Policy 加为 required check，并绑定 GitHub Actions source。
7. 运行人工合并 canary。
8. 仅对 canary allowlist 启用 Finalizer。
9. 迁移 Sync 使用同一 Writer App；同步分支/PR 写入用 Writer token，冲突和 state/policy drift 的 Issue/comment 告警用 `GITHUB_TOKEN`。
10. 观察稳定窗口后删除旧 policy/merger/sync Environment、变量和废弃 PR。

紧急回滚顺序：

1. 关闭 Agent/Writer/Finalizer 开关。
2. 枚举并 disarm 所有 managed PR；必要时关闭。
3. suspend/uninstall Writer App，轮换私钥。
4. 保留 required Policy，不通过删门禁恢复服务。
5. 通过人工受保护 PR revert 自动化代码。
6. 临时恢复人工上游同步。

软 kill switch 不撤销已签发 token，也不取消已经 armed 的 auto-merge，因此第 2、3 步不可省略。

## 12. 实施文件边界

计划新增：

- `.github/workflows/agent-candidate.yml`
- `.github/workflows/automation-policy.yml`
- `.github/workflows/autonomy-finalizer.yml`
- `.github/automation/src/autonomy-*.mjs`
- `.github/automation/test/autonomy-*.test.mjs`
- `.github/codex/prompts/implement.md`
- `.github/codex/schemas/result.schema.json`
- `docs/single-writer-autonomy-runbook.md`

计划修改：

- `.github/agents.yml`
- `.github/automation-policy.yml`
- `.github/CODEOWNERS`
- `.github/workflows/frontend-ci.yml`
- `.github/workflows/sync-upstream.yml`
- `.github/workflows/scripts/sync-upstream.sh`
- `docs/automation-architecture.md`
- `docs/development-workflow.md`

所有 workflow 和第三方 Action 固定完整 commit SHA。每个生产开关默认关闭；GitHub 设置 mutation 必须有 mutation 前快照和可执行回滚记录。实施文件已在当前工作树中出现不等于远端设置、production flags 或 required Policy 已启用。
