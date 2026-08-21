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
6. `main` 严格要求 Rust、Frontend、Policy 和 `Autonomy Finalizer / hold` 四个检查，且来源绑定 GitHub Actions。
7. Finalizer 不直接 merge，只在精确状态复核后请求 native squash auto-merge。
8. 初始无人合并 allowlist 仅覆盖 `docs/automation-canary/**/*.md`。
9. 上游同步复用同一个 Writer App、固定分支和单一开放 PR。
10. 冲突、状态漂移、敏感路径、读取不完整和凭据异常全部 fail closed。
11. `release` Environment 仍需人工审批且管理员不可绕过。
12. 六项 GitHub 现场 PoC、fork PR canary、Actions token branch-protection 读取 canary 和撤销演练均有 run、PR、SHA 和 API 快照证据。

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
  -> Autonomy Finalizer / hold（精确 head；受管 PR 保持 pending）
  -> Finalizer（API-only；不 checkout PR head）
  -> hold success（仅在 native auto-merge 独立回读确认后）
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
| Hold initializer | `GITHUB_TOKEN` | checks write、contents/PR read | 无 | checkout/执行 PR head、Writer 凭据 |
| Finalizer | `GITHUB_TOKEN` + Writer App token | Actions token 仅写 hold check；Writer 仅 PR/contents write | App private key | checkout PR、直接 merge、admin bypass |
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

Policy 不轮询或汇总 Rust/Frontend。GitHub 直接要求四个独立 contexts：

- `Rust CI / check`
- `Frontend CI / check`
- `Automation Policy / gate`
- `Autonomy Finalizer / hold`

前三项是业务门禁；hold 是 native auto-merge 的服务端时序屏障。Expected source 只能绑定 GitHub Actions App，不能区分具体 workflow。因此 Policy 和 hold workflow 必须来自 base 分支，且治理路径不能进入无人合并 allowlist。

## 9. Finalizer 实时复核

Finalizer 在 Policy workflow 完成后运行，但只有以下条件全部成立才行动：

1. 受信 workflow 名称、event、repository ID、base branch 匹配。
2. PR 仍开放，head/base SHA 与触发快照一致。
3. head repo 为本仓库、branch 为 `agent/issue-*`；受管同步走独立确定性流程。
4. PR author 为配置的 Writer App bot。
5. Policy 对当前 diff 重新计算为 `eligible`。
6. 当前 head 上的 Policy check 为 GitHub Actions expected source 且成功。
7. branch protection 仍是 strict，并包含四个 required checks 及正确 source。
8. 无冲突、无阻塞 review、无未解决讨论、无 manual-only 标签。
9. auto-merge 开关和授权有效期仍有效。

安全时序固定如下：

1. base 受信的 API-only initializer 为 `agent/issue-*` 受管 PR 的精确 head 创建 `in_progress` hold；普通非 Writer PR 获得 success，避免全局阻塞正常 PR；Writer App 仅有精确 `automation/sync-upstream` 分支及完整同步 marker 可直接获得 success，其他 Writer 分支或 malformed metadata 一律 failure。
2. Finalizer 复核前三项业务检查、GraphQL Bot 身份和 strict branch protection，并确认 hold 已作为第四项 required check 绑定 GitHub Actions App。固定版本的 token action 必须证明 mint 所用 App ID、实时 App slug、配置 installation ID 与实时 installation ID 一致；installation token 还必须独立证明只可访问当前仓库。
3. Finalizer 再次确认 exact-head hold pending 后才把 Draft PR 转为 ready；此时 `mergeStateStatus` 必须为 `BLOCKED`。
4. Writer token 只调用 `enablePullRequestAutoMerge`，方法固定 `SQUASH` 并携带 `expectedHeadOid`；不存在直接 merge 路径。
5. mutation 返回值不作为授权证据。Finalizer 通过独立 GraphQL 读取确认相同 PR/head 已持久化 `autoMergeRequest`。
6. 最后才把同一个 hold check 完成为 success；GitHub 随后按 native auto-merge 和实时保护状态决定是否合并。

任一步骤失败时 hold 保持 pending。若 arming 明确未生效，可恢复 Draft；若远端结果不确定或已 armed，则不猜测性回滚或释放 hold，由下一次可信事件重入收敛。

## 10. PoC 顺序

1. **事件语义**：用 disposable PR 分别验证 `GITHUB_TOKEN` 事件抑制和 Writer App `pull_request` CI 触发。
2. **身份与权限**：验证受管分支/PR 成功，`main`、workflow、checks、statuses、admin 和 release 写入失败；记录其他未保护分支的真实残余能力。
3. **Artifact 隔离**：篡改 schema、digest、repo/base/task/path/mode/大小，证明均在 token mint 前失败。
4. **Policy 来源**：将 Policy 加为 required check 并绑定 GitHub Actions；用其他来源同名状态证明不能满足保护。
5. **hold 与 native auto-merge**：先 shadow 初始化并为所有开放 PR 回填 hold，再把 hold 绑定为 required check；逐一构造 head/base/check/draft/conflict/discussion 漂移和 mutation 响应丢失，证明不会直接 merge 或绕过 pending hold。额外以 fork PR 验证 `pull_request_target` initializer 不 checkout 或执行 fork head，且能在精确 head 上发布正确来源的 hold；以独立 Actions `GITHUB_TOKEN` canary 验证可读取目标分支 protection/ruleset。两项均为上线前阻断证据，未取得 run、PR、SHA 和 API 回读快照前不得声称已完成。
6. **同步幂等**：连续三轮验证固定分支、最多一个开放 PR和 no-op；冲突、unknown tip、历史重写、人工关闭全部停止。
7. **撤销**：关闭变量、disarm managed PR、suspend App、轮换 key，再验证无新写入。
8. **Release**：触发 release lane，确认仍等待维护者审批且无 Agent secret。

PoC 使用 disposable issue、`docs/automation-canary/` 和 Draft PR。当前 Draft PR 与 Policy required check PoC 已完成；在 hold 回填、source 绑定和 native auto-merge canary 全部通过前，`AERIS_WRITER_ENABLED=false`、`AERIS_UPSTREAM_SYNC_ENABLED=false`、`AERIS_AUTONOMOUS_MERGE_ENABLED=false`，Finalizer 不请求 auto-merge。不能因仓库内 workflow 已存在而声称远端 hold、ruleset 或 canary 已完成。

上线前必须记录每次 canary 的 `main` base SHA、PR base/head SHA、required contexts 快照和 GraphQL/REST 读取时间。若 `main` 推进、PR base 更新或 required context/source 漂移，当前 run 标记为 stale 并停止，必须以新 base SHA 重跑，不能沿用旧 Policy/hold 结论。以下远端能力是上线阻断条件：

- Finalizer 使用的 Actions `GITHUB_TOKEN` 实测可以读取当前仓库 GraphQL `branchProtectionRules`，并看到 strict、required status checks 及 GitHub Actions source；权限不足或读取不完整时保持 hold pending。
- disposable fork PR 实测 `pull_request_target` initializer 能在 fork head SHA 创建并独立回读 `Autonomy Finalizer / hold` Check Run。若 GitHub 拒绝该 head，fork PR 必须进入明确的 fail-closed/manual 路径，不得把缺失 hold 当作 success，也不得启用 hold required context。

malformed terminal hold（错误 `head_sha`、`external_id`、name、重复检查，或已 completed 但结论/auto-merge 不匹配）不可复用、覆盖或手工改写为 success。initializer/finalizer 必须报告冲突并保持 PR 不可合并；由受信 Publisher 生成新精确 head 后创建新 hold，固定分支上的现有 PR 保持开放，不能为恢复而关闭。远端响应丢失时不得根据失败响应猜测状态，必须独立回读；无法确认则保持 fail closed，禁止人工补写绕过时序屏障。

## 11. 上线与回滚

上线顺序：

1. 合入 disabled 代码和测试。
2. Policy shadow 运行并核对分类。
3. 创建并仅安装一个 Writer App。
4. 配置 `agent` Environment（无人工审批，仅模型凭据）和 `writer` Environment（App ID/私钥），并记录远端设置快照；`release` Environment 继续人工审批。
5. 运行 Draft canary，不启用 auto-merge。
6. 完成并保留 fork PR initializer canary 与 Actions `GITHUB_TOKEN` branch-protection/ruleset 读取 canary 的 run、PR、SHA、API 回读快照；任一失败均不得继续修改保护规则。
7. PoC 通过后将 Policy 加为 required check，并绑定 GitHub Actions source。
8. 以 shadow 模式运行 hold initializer，为全部开放 PR 精确回填；确认受管 PR pending、非受管和 fork PR success，并回读每个 check 的 exact head、external ID 和 GitHub Actions source。
9. 将 hold 加为第四项 strict required check 并绑定 GitHub Actions source；回读配置后才允许 Finalizer mint Writer token。
10. 仅对 canary allowlist 启用 Finalizer并完成 native auto-merge canary。
11. 迁移 Sync 使用同一 Writer App；同步分支/PR 写入用 Writer token，冲突和 state/policy drift 的 Issue/comment 告警用 `GITHUB_TOKEN`。
12. 观察稳定窗口后删除旧 policy/merger/sync Environment、变量和废弃 PR。

### PR #72 的安全复用

当前 PR #72 的 base SHA 已落后于 `main`，不能把既有 head、checks 或 base 快照当作可复用授权。不得关闭该 PR，以免触发 managed branch tombstone。仅可通过 GitHub 的安全 retarget/更新路径让同一个开放 PR 的 base 与当前 `main` 一致，然后实时双读 PR 的 number、state、head ref/OID、base ref/OID、head repository、author、Draft 状态及 required checks；只有 base OID 与当时 `main` 精确相等、所有 check 都对应新 head，且 Finalizer hold 仍为新 head 的 pending 状态，才能继续复用。任一更新冲突、API 结果不完整、回读不一致或 base 再次漂移时停止，保留 Draft 并重新走受管发布/验证，不关闭 PR 或猜测性 force-push。

紧急回滚顺序：

1. 立即关闭 Agent、Writer、Finalizer 和 upstream sync 开关，阻止新的 token mint、分支/PR 写入和 native auto-merge 请求。
2. 完整分页枚举所有 managed PR，逐一 disarm native auto-merge；远端结果不确定或 disarm 未确认时，保持该 PR Draft 和 hold pending，直到独立回读确认 `autoMergeRequest == null`。必要时关闭这些 PR，但不得删除固定分支留下不可审计状态，也不得为了回滚而释放 managed PR 的 hold。
3. 在 managed PR 已 disarm 且复核完成后，移除 `Autonomy Finalizer / hold` required context，回读 branch protection/ruleset 并确认该 context 已不再 required，且 Rust、Frontend、Policy 等门禁与来源绑定未被弱化；不通过一次性删除全部保护来恢复服务。
4. 仅在第 3 步回读确认后，才通过人工受保护 PR revert/remove initializer 和 finalizer workflow。若选择保留 initializer，它必须继续只运行 base 受信代码、只创建/完成 hold Check Run、不持有 Writer secret，并保持生产开关关闭；保留它用于后续回填或恢复时，仍需单独验证 required context 已移除且不会触发写入。
5. 如需彻底停用，suspend/uninstall Writer App 并轮换私钥；记录 installation 404/列表快照。该步骤不能替代第 2 步的 disarm 确认。
6. 临时恢复人工上游同步，并保留回滚前后的配置、PR、SHA 和 API 证据。

恢复自动化时按相反依赖关系执行：先合入并启用 initializer（和 disabled 的 Finalizer 实现），为开放 PR 产生真实的、精确 head、来源为 GitHub Actions 的 `Autonomy Finalizer / hold` check，并逐项 API 回读；只有该证据成立后，才把该 context 重新加入 strict required checks。不得先把 hold 加回 required，再部署 initializer 或依赖历史 check。

软 kill switch 不撤销已签发 token，也不取消已经 armed 的 auto-merge，因此第 2、3、5 步不可省略。

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
