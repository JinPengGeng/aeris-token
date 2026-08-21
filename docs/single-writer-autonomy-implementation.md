# 单 Writer App 自主开发实施规格

> 状态：`implementation-in-progress / production-disabled`
> 日期：2026-08-20
> 目标仓库：`JinPengGeng/aeris-token`
> 设计依据：`docs/research/github-autonomous-automation-architecture-research-2026-08-20.md`

## 1. 目标与完成定义

本方案在 GitHub 内运行，不依赖本地电脑常驻。AI 负责生成和测试候选变更；一个最小 Writer App 负责受控分支和 Pull Request 写入；GitHub Actions、分支保护和一次性的 GitHub GraphQL direct squash merge 负责确定性门禁与最终合并。

只有以下证据全部成立，才可声明实现完成：

1. Agent job 不持有 GitHub 写凭据、Writer App 私钥或 release secret。
2. Publisher 不执行候选代码，只验证并应用有界 patch。
3. Writer App 仅安装到本仓库，仅有 `administration:read`、`contents:write` 和 `pull_requests:write`；没有任何 Administration 写权限或 bypass。
4. PR CI 使用 `pull_request`、只读 token、无 secrets。
5. `Automation Policy / gate` 来自 GitHub Actions，并由 `main` 的受信策略代码计算。
6. `main` 严格要求 Rust、Frontend 和 Policy 三个检查，且来源绑定 GitHub Actions；不新增 Finalizer hold context。
7. Finalizer 只在两次完整复核之间执行一次 `mergePullRequest(SQUASH, expectedHeadOid)`；不设置 native auto-merge 或任何其他持久合并授权。
8. 初始无人合并 allowlist 仅覆盖 `docs/automation-canary/**/*.md`。
9. 上游同步复用同一个 Writer App、固定分支和单一开放 PR。
10. 冲突、状态漂移、敏感路径、读取不完整和凭据异常全部 fail closed。
11. `release` Environment 仍需人工审批且管理员不可绕过。
12. 六项 GitHub 现场 PoC、Writer token 完整治理读取 canary、direct-merge 响应不确定回读 canary 和撤销演练均有 run、PR、SHA 和 API 快照证据。

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
  -> Finalizer preliminary proof（Actions token；不读取 branch protection）
  -> mint 单仓 Writer token + 身份/权限范围证明
  -> Finalizer full proof（Writer administration:read）
  -> Ready 前 full proof -> Draft 转 Ready -> Ready 后 full proof
  -> Finalizer（API-only；不 checkout PR head；一次 GraphQL direct squash merge）
  -> protected main
```

上游同步走独立确定性入口，但复用 Writer App：

```text
schedule / workflow_dispatch
  -> checkpoint 三方合并
  -> automation/sync-upstream
  -> 单一 managed PR
  -> bounded required-check success gate
  -> 一次 exact-head REST squash merge + strict readback
  -> 上游同步的独立合并路径（不设置持久授权）
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
| Finalizer | `GITHUB_TOKEN` + Writer App token | Actions token 读取 CI/PR；Writer 仅 `administration:read`、PR/contents write | App private key | checkout PR、native auto-merge、admin write/bypass |
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
| `eligible` | success | 可继续实时复核并执行一次 direct squash merge |
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

Expected source 只能绑定 GitHub Actions App，不能区分具体 workflow。因此 Policy workflow 必须来自 base 分支，且治理路径不能进入无人合并 allowlist。Finalizer 不增加第四 required context，也不留下 terminal hold。

## 9. Finalizer 实时复核与一次性合并

Finalizer 在 Policy workflow 完成后运行，但只有以下条件全部成立才行动：

1. 受信 workflow 名称、event、repository ID、base branch 匹配。
2. PR 仍开放，head/base SHA 与触发快照一致。
3. head repo 为本仓库、branch 为 `agent/issue-*`；受管同步走独立确定性流程。
4. PR author 的 GraphQL `Bot` login、database ID 和 node ID 均与 Writer token 实时读取的 REST Bot 身份一致。
5. Policy 对当前 diff 重新计算为 `eligible`。
6. 当前 head 上的 Policy check 为 GitHub Actions expected source 且成功。
7. branch protection 的 review、deployment、push、lock、signature、linear-history、admin、bypass、conversation、ruleset 和三个 required checks 均与受审计 profile 精确一致。
8. 无冲突、无阻塞 review、无未解决讨论、无 manual-only 标签。
9. Writer App、installation、单仓 scope、Bot 身份和 `administration:read` 治理读取逐项成立，且授权有效期仍有效。

安全时序固定如下：

1. Finalizer 的 mint 前步骤只执行显式 `preliminary` proof，验证 trigger、Policy、PR governance 和三项业务检查，绝不读取 branch protection/ruleset；其结果只决定是否值得进入 Writer Environment 和 mint token，不构成合并资格证明。
2. mint 后使用同一枚、仅限当前仓库的 Writer installation token，以 `administration:read` 执行 `full` proof。固定版本的 token action 必须证明 mint 所用 App ID、实时 App slug、配置 installation ID 与实时 installation ID 一致；installation token 还必须独立证明只可访问当前仓库，并以 `GET /users/{slug}[bot]` 取得实时 REST login、database ID、node ID。PR author 必须与这三个字段一致。
3. Draft 转 Ready 前重新执行 full proof；仅当其仍绑定精确 head、最新 base、三项 required checks、讨论、review、Policy、分支保护和 Writer App 治理投影时才转 Ready。转 Ready 后再次执行同样的 full proof。
4. 仅当第二次 full proof 仍成立时，Writer token 才调用 GitHub GraphQL `mergePullRequest`，固定 `mergeMethod: SQUASH` 和 `expectedHeadOid`。Finalizer 不调用 `enablePullRequestAutoMerge`，也不创建 hold 或其他可由未来状态变化触发的持久授权。
5. mutation 明确失败时 PR 保持未合并；网络超时、连接中断或其他响应不确定时不重试 mutation，而是独立读取 PR 合并结果。只能确认同一 PR、同一 head 已合并才记为成功，无法确认即 fail closed。已存在的 native auto-merge 或任何身份/治理漂移均会在 Writer mutation 前拒绝。

该路径仍不能把维护者、其他 App 或 GitHub 对 label、review、discussion、check 和保护设置的并发变更锁入事务；但 `expectedHeadOid` 把 merge 限定到复核的 head，失败不留下持久授权，未知响应只经独立回读收敛。任一读取不完整、TOCTOU 漂移或回读不确定都停止，由下一次可信事件从头重算。

## 10. PoC 顺序

1. **事件语义**：用 disposable PR 分别验证 `GITHUB_TOKEN` 事件抑制和 Writer App `pull_request` CI 触发。
2. **身份与权限**：验证受管分支/PR 成功，`main`、workflow、checks、statuses、admin 和 release 写入失败；记录其他未保护分支的真实残余能力。
3. **Artifact 隔离**：篡改 schema、digest、repo/base/task/path/mode/大小，证明均在 token mint 前失败。
4. **Policy 来源**：将 Policy 加为 required check 并绑定 GitHub Actions；用其他来源同名状态证明不能满足保护。
5. **direct merge**：逐一构造 head/base/check/draft/conflict/discussion/治理漂移，以及 `mergePullRequest` 响应丢失，证明不会合并错误 head、不会创建持久授权，且响应不确定时只通过独立回读收敛。canary 必须记录 Writer Bot 的 REST/GraphQL login、database ID、node ID，与 PR author 对照；另验证 Writer installation token 的 `administration:read` 可读取目标分支完整治理 profile，并证明 Actions token preliminary proof 的 protection 读取次数为零。未取得 run、PR、SHA 和 API 回读快照前不得声称已完成。
6. **同步幂等**：连续三轮验证固定分支、最多一个开放 PR和 no-op；冲突、unknown tip、历史重写、人工关闭全部停止。
7. **撤销**：关闭变量、枚举 managed PR、确认 Finalizer 未留下持久授权、suspend App、轮换 key，再验证无新写入。
8. **Release**：触发 release lane，确认仍等待维护者审批且无 Agent secret。

PoC 使用 disposable issue、`docs/automation-canary/` 和 Draft PR。当前 Draft PR 与 Policy required check PoC 已完成；在 source 绑定和 direct-merge canary 全部通过前，`AERIS_WRITER_ENABLED=false`、`AERIS_UPSTREAM_SYNC_ENABLED=false`、`AERIS_AUTONOMOUS_MERGE_ENABLED=false`，Finalizer 不执行 direct merge。不能因仓库内 workflow 已存在而声称远端 ruleset 或 canary 已完成。

上线前必须记录每次 canary 的 `main` base SHA、PR base/head SHA、三个 required contexts 快照和 GraphQL/REST 读取时间。若 `main` 推进、PR base 更新或 required context/source 漂移，当前 run 标记为 stale 并停止，必须以新 base SHA 重跑，不能沿用旧 Policy/proof 结论。以下远端能力是上线阻断条件：

- Writer App 已授予且仅授予 `administration:read`，Finalizer mint 的单仓 installation token 实测可以读取当前仓库 GraphQL `branchProtectionRules`/rulesets，并看到完整治理 profile、三项 strict required status checks 及 GitHub Actions source；权限不足、权限超出或读取不完整时 fail closed。无密钥 preliminary proof 不得作为完整授权证明。

已存在的 native auto-merge、错误 `head_sha` 或其他 PR 身份/治理投影漂移均不可接纳或手工补写为成功。Finalizer 必须报告冲突并保持 PR 未合并；远端响应丢失时不得根据失败响应猜测状态，必须独立回读；无法确认则保持 fail closed。

## 11. 上线与回滚

上线顺序：

1. 合入 disabled 代码和测试。
2. Policy shadow 运行并核对分类。
3. 创建并仅安装一个 Writer App。
4. 配置 `agent` Environment（无人工审批，仅模型凭据）和 `writer` Environment（App ID/私钥），并记录远端设置快照；`release` Environment 继续人工审批。
5. 将同一 Writer App 的 Repository permissions 增加为 `Administration: read-only`，接受 installation 的权限更新，并以 API 回读确认没有 Administration 写权限或额外仓库范围；随后运行 Draft canary，仍不执行 direct merge。
6. 完成并保留 Writer token branch-protection/ruleset 完整读取 canary 与 direct-merge 不确定响应回读 canary 的 run、PR、SHA、API 回读快照，并证明 preliminary proof 未调用保护规则 API；任一失败均不得继续修改保护规则。
7. PoC 通过后将 Policy 加为 required check，并绑定 GitHub Actions source。
8. 回读 `main` 保护配置，确认只有 Rust、Frontend、Policy 三项 strict required checks 且来源绑定 GitHub Actions；不得加入 Finalizer hold context。
9. 仅对 canary allowlist 启用 Finalizer 并完成一次 direct squash merge canary。
10. 迁移 Sync 使用同一 Writer App；同步分支/PR 写入与一次 exact-head REST squash merge 用 Writer token，冲突和 state/policy drift 的 Issue/comment 告警用 `GITHUB_TOKEN`。Sync 只在本轮有界 required-check gate 成功后发起一次 `PUT /pulls/{number}/merge`（`merge_method=squash`、精确 `sha`）；无论 mutation 响应如何都只独立回读一次，无法证明 Writer bot 对同一 head/base 产生单 parent squash commit 且 `auto_merge=null` 时 fail closed，保留开放 managed PR 供后续同步复用，不设置 native auto-merge。
11. 观察稳定窗口后删除旧 policy/merger/sync Environment、变量和废弃 PR。

### PR #72 的安全复用

2026-08-21 已通过 GitHub `update-branch` 安全更新同一个开放 PR #72：head 从 `17bf0c20961fa8033674a6668b76c2ce96c919eb` 更新为 `3d94425470209e0116848b19262c9526690c8226`，`baseRefOid/base.sha` 同步刷新为当时 `main` 的 `8355f83b7bc27865c741f6b2aa0c11cb1f04a73d`，compare 回读为 `behind_by=0`。旧 head 的 checks 不再作为授权；新 head 的 Rust CI 首轮命中 `truncated_sse_is_a_partial_body_error_over_h2c` 波动失败，failed-jobs 重跑后成功。PR 仍保持 `OPEN + Draft` 且 `autoMergeRequest=null`，未关闭、未 force-push。后续复用仍必须实时双读 PR 的 number、state、head ref/OID、base ref/OID、head repository、author、Draft 状态及三个 required checks；只有 base OID 与当时 `main` 精确相等、所有 check 都对应新 head，且两次 Finalizer full proof 都成功，才能继续。任一更新冲突、API 结果不完整、回读不一致或 base 再次漂移时停止，保留 Draft 并重新走受管发布/验证。

紧急回滚顺序：

1. 立即关闭 Agent、Writer、Finalizer 和 upstream sync 开关，阻止新的 token mint、分支/PR 写入和 direct merge。
2. 完整分页枚举所有 managed PR，确认 Finalizer 没有留下 native auto-merge；必要时关闭开放 PR，但不得删除固定分支留下不可审计状态。
3. 回读 branch protection/ruleset，确认 Rust、Frontend、Policy 三项门禁与来源绑定未被弱化；不通过一次性删除全部保护来恢复服务。
4. 通过人工受保护 PR revert/remove Finalizer workflow；不保留或恢复 hold initializer。
5. 如需彻底停用，suspend/uninstall Writer App 并轮换私钥；记录 installation 404/列表快照。
6. 临时恢复人工上游同步，并保留回滚前后的配置、PR、SHA 和 API 证据。

恢复自动化时先合入 disabled 的 Finalizer 实现，回读 `main` 仍只要求三项 strict required checks，再从 Draft canary 重做 preliminary/full proof 与 direct-merge 验证；不得引入或依赖历史 Finalizer hold。

软 kill switch 不撤销已签发 token，因此第 2、3、5 步不可省略；但 Finalizer 的 direct merge 不会留下待未来触发的合并授权。

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

## 13. Writer App 硬到期撤销

`.github/workflows/autonomy-expiry-revoker.yml` 每 5 分钟运行一次，并在
`AERIS_AUTONOMY_EXPIRES_AT` 前 45 分钟进入撤销阶段；所有 Writer token mint 在到期前
60 分钟停止，因此撤销前至少留出 15 分钟静默期。它在 `writer`
Environment 中使用同一个 `AERIS_WRITER_APP_PRIVATE_KEY` 生成短期 App JWT，严格校验
App ID、slug、installation account、repository ID 和 repository 名称。App name、URL、webhook、
events、slug、permission 集合、installation repository selection 等可变元数据即使漂移也不能阻止撤权；
App ID 是管理身份锚点，实时 slug 与配置 slug 会同时用于捕获 rename 前后创建的 Writer PR。

PR inventory 以不可伪造的作者身份为边界：Writer App bot 创建的所有开放 PR 都进入撤销范围，
即使 marker、head、base 或 issue 元数据已经漂移；legacy sync 只接受精确 legacy bot、同仓库 head
和 `automation/sync-upstream`。外部作者伪造 marker 或分支名只作为不可信提示忽略，不能阻断卸载。
每轮完整分页连续读取两次并比较全部 PR identity；mutation 后还要求连续两轮受管 inventory 一致且
`autoMergeRequest == null`。

Publisher、Finalizer、Sync 和 Revoker 的 mutation job 共用 `aeris-writer-mutation` concurrency lock。
撤销先使用仅具 `contents: read`、`pull-requests: write` 的 workflow `GITHUB_TOKEN` 做 pre-disarm，
随后用 App JWT 卸载 installation 并通过 404 和 installation 列表复核，再以独立的 workflow token
做 post-uninstall 收敛；因此最后一次复核前发生的 rearm 或新 Writer PR 仍会被捕获。pre-disarm
具有 60 秒总预算，超时即继续 uninstall，不能耗尽 10 分钟 runner deadline；post-disarm 具有独立
180 秒预算。DELETE 响应、卸载确认或 installation 列表失败均不能跳过 post-disarm，最终会聚合报告
撤权与 cleanup 状态。pre-disarm 失败不能保留 Writer authority；post-disarm 失败则明确报告 cleanup incomplete。旧 installation ID
返回 404 时会重新发现目标 account 上的 installation，稳定重复执行返回 `already_uninstalled`。

`workflow_dispatch` 默认执行同一撤销路径；检查演练必须显式选择 `dry_run`。`force` 只绕过时间窗口，
且要求 Agent、Writer、Sync 和 autonomous merge 四个生产开关均已关闭。启用生产前必须通过 disposable
PR 实测 `GITHUB_TOKEN` 的 GraphQL `disablePullRequestAutoMerge` 权限；该 canary 是上线阻断条件，
因为 GitHub 没有为该 mutation 提供可静态证明的细粒度权限表。该 token 不能写 contents，且
`GITHUB_TOKEN` 无法删除 repository/environment secrets 或 variables，因此密钥记录仍需后续人工清理；
installation 卸载后，该密钥无法再获得仓库访问权，除非人工重新安装 App。GitHub schedule 延迟、
GitHub API 故障，以及没有服务端 required lease check 时 pre-disarm 到 uninstall 之间仍可能完成 merge，
是必须保留并监控的残余边界。

## 14. Writer 现场证明与 response-loss canary 运维

在启用 Writer mutation 前，从 default branch 手工运行
`.github/workflows/writer-readonly-attestation.yml`。该 workflow 没有输入，只进入受保护的 `writer`
Environment；workflow `GITHUB_TOKEN` 仅有 `contents: read`，临时 installation token 也显式缩窄为
`administration: read`、`contents: read`、`pull-requests: read`。运行必须证明 App/installation 的
ID、slug、owner、未暂停、`repository_selection=selected`、精确 Writer 权限、仅当前一个仓库可访问，
以及 REST/GraphQL Bot 身份一致；它不执行 Git、PR、Issue 或其他写 mutation。

Finalizer response-loss live canary 仅通过 repository variable
`AERIS_FINALIZER_RESPONSE_LOSS_CANARY` 配置，值必须是无额外字段的单行 JSON：

```json
{"version":1,"fault":"drop_merge_response_after_success","pull_number":17,"head_sha":"<40hex>","base_sha":"<40hex>"}
```

`pull_number` 必须绑定 disposable 目标 PR，`head_sha` 和 `base_sha` 分别取该次 eligibility snapshot
的精确 head 与 default-branch base。畸形 JSON、未知字段、错误版本/fault 或非法 SHA 一律 fail closed；
PR 号与当前 PR 相同时，任一 SHA 不同也 fail closed，防止目标 PR 静默退化为普通合并。结构合法但
PR 号不同的历史绑定保持 dormant，不激活 canary、不阻断其他 PR，也不输出 dormant 日志。精确匹配时，
Finalizer 只在一次 GraphQL squash merge 响应已经返回后丢弃该响应，随后仅用独立 readback 接受精确
merged outcome；open、未知或歧义结果均失败，且不得第二次调用 merge。

Canary 完成或失败后立即删除 `AERIS_FINALIZER_RESPONSE_LOSS_CANARY`，不能把 dormant 语义当作长期配置。
保存只读 attestation 与 Finalizer 的 run URL、job log、step summary、非敏感 App/installation 权限与
repository/Bot API 证明，以及 `AERIS_FINALIZER_CANARY=response_loss_after_merge_response` marker；不得保存
private key 或 installation token。删除变量后再保存变量缺失截图或 API 查询结果，作为现场清理证据。
