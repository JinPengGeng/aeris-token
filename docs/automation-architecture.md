# GitHub 自动化与 Agent 架构

本文定义 `aeris-token` 的上游同步、Issue 分析、Agent 协作和条件自动合并边界。它是实施契约，不代表所有能力已经启用。各阶段只有在对应 workflow、测试和仓库设置完成后才生效。

实施总入口为 GitHub Issue `#11`。日常开发流程仍以 [development-workflow.md](development-workflow.md) 为准。

## 1. 设计原则

1. `main` 继续保持 squash-only 和线性历史。
2. 自动化只通过分支和 Pull Request 修改代码，不直接写入 `main`。
3. Issue、评论、PR 内容和模型输出均是不可信输入。
4. 模型负责分析和建议；授权、路径限制和合并由确定性规则决定。唯一的冲突自动化例外是受限上游同步中逐个满足 UTF-8、普通文件 mode `100644`、两侧均为 modify 的文本冲突：无 GitHub 写凭据的 AI Resolver 只能生成实际候选 artifact，不同 model ID 的独立 AI Reviewer 必须审查该 artifact；trusted deterministic verifier 只有在最终 attestation 精确绑定 artifact 链、当前 head/tree、base、checkpoint、upstream 和 policy 后，才允许 Writer 执行一次 server-side direct squash。任何不满足该范围或任一验证失败的冲突仍 fail closed，并转人工处理。
5. 唯一的 GitHub 写入身份是最小 Writer App；Policy 是 GitHub Actions 的确定性检查，Finalizer 只在实时复核后调用一次 GitHub GraphQL direct squash merge，不存在独立 Policy 或 Merger App，也不存在 Finalizer 持久 auto-merge 授权。
6. 所有策略从受保护的 `main@SHA` 读取，不能从当前 PR checkout 读取。
7. GitHub 事件按可能重复、乱序和重试处理；Actions-only 阶段只提供 best-effort 去重和收敛，不声明 strict exactly-once。
8. 功能按只读分析、Draft PR、Policy Gate、自动合并的顺序逐级开放。

## 2. 当前阶段

仓库内的策略源为：

- `.github/agents.yml`：Agent registry、模型路由和逻辑能力。
- `.github/automation-policy.yml`：授权、限额、幂等、安全和合并门禁。
- `.github/upstream-sync-policy.yml`：上游身份、路径所有权和 checkpoint 约束。

仓库文件中的生产开关默认关闭；这不代表 GitHub 远端的 Variables、Environment、App、ruleset 或 required checks 已完成配置：

```text
AERIS_AGENTS_ENABLED != true
```

Phase 2.1 采用 Actions-only，不部署常驻 Coordinator 或外部数据库。每条只读 workflow 分成四个权限隔离的 job：`preflight` 读取受信配置并验证调用，`reserve` 对 managed comment 做 best-effort 预约，`analyze` 以只读 GitHub 权限调用模型，`publish` 确定性写回。只有真正进入 reserved 分支的 `analyze` step 接收 AI Key；terminal passthrough step 不接收 Secret。阶段间只传递经过 schema 校验的有界 JSON artifact，运行器不会把输入正文或 PR patch 直接复制进 artifact；`analyze` 重新读取输入并核对 fingerprint，`publish` 再次获取目标输入并复核 fingerprint 后更新 managed comment。artifact 不得包含 Secret、Authorization、Cookie 或请求头，并受 registry 中的字节上限约束；模型输出还会在 artifact 写入前拒绝当前 AI Key 和常见认证头/Bearer 形态，但这属于针对已知运行 Secret 的防泄漏护栏，不是通用 DLP。结构化模型输出仍可能概括或引用输入内容，因此 artifact 按目标仓库数据同级处理。`reserve` 不是强租约，artifact 也不是锁或权威状态源。

只读 Agent 仍保持 Actions-only：Candidate job 只持有 `contents:read` 和模型凭据，不持有 GitHub 写凭据、Writer App 私钥或 release secret。单 Writer 实现已在工作树中加入 Candidate、Publisher、Policy 和 Finalizer workflows；在 GitHub 现场 PoC、Environment/secret 配置、branch protection 和 required Policy 检查均有证据前，不能把它们描述为已在远端启用。`agent` Environment 必须不设置人工审批，且不得暴露 Writer 或 release secret；`writer` Environment 用于受保护的 Writer App 凭据；`release` Environment 继续保持人工审批。

## 3. 信任和配置来源

每次运行必须先取得默认分支的精确 SHA，并通过 GitHub API 从该 SHA 加载 registry 和 policy。以下来源不能影响本次授权：

- Issue 或评论中给出的模型名、路径规则或权限声明。
- 当前 PR 对 `.github/**` 的修改。
- 模型返回的工具参数、shell 命令或后续 Agent 名称。
- 未经 registry 允许的 webhook actor、命令或 handoff。

策略文件、同步状态、CODEOWNERS 和 workflows 是不可由 Writer 修改的控制面文件。涉及这些文件的变更必须由维护者审查。

## 4. AI 接口与模型路由

OpenAI 兼容接口通过 GitHub Actions 配置：

| 类型 | 名称 | 用途 |
| --- | --- | --- |
| Secret | `AERIS_AI_API_KEY` | Bearer token，不得进入日志或 artifact |
| Variable | `AERIS_AI_BASE_URL` | 包含 `/v1` 的 HTTPS 基础地址 |
| Variable | `AERIS_AI_MODEL` | 默认模型 |
| Variable | `AERIS_AI_MODEL_<ROLE>` | 角色专用模型 |
| Variable | `AERIS_AI_MODEL_FALLBACK` | 瞬时失败时的回退模型 |

默认请求地址为：

```text
${AERIS_AI_BASE_URL}/chat/completions
```

模型解析顺序为角色专用模型、默认模型、fallback。实际模型 ID 必须来自 registry 声明的 GitHub Variables；用户输入不能直接指定任意模型 ID。

Fallback 仅适用于连接失败、超时、429 和 5xx。认证失败、权限拒绝、请求错误、模型不存在和策略拒绝必须直接停止。客户端契约包含与总超时对齐的 120 秒连接/响应头窗口（远端实测配置的推理模型首字节延迟在 30-60 秒以上且随任务复杂度上升：triage 32s 超时、planner 62.4s 超时，见 run `31811444818`、`31815519729`、`31819708321`；`/models` 在同一 runner 约 1 秒返回，排除网络因素，因此连接窗口从 10 秒经两次调参最终与 120 秒总请求超时对齐）、120 秒总请求超时和 1 MiB 响应上限；连接阶段超时可以进入受控 fallback，总超时仍覆盖响应体读取。每次调用记录 Agent、模型别名、模型 ID、耗时和用量，不记录 Key、Authorization 或 Cookie。

模型调用或输出 schema 失败时，`analyze` 生成受约束的 `failed` artifact，`publish` 清理预约并把失败原因写入 managed comment。该预期失败路径允许四阶段 workflow 正常完成，避免只读 advisory 自动化成为 required CI 的阻断项；运行状态应从 managed comment 和 Actions 日志观察，而不是把 workflow 绿色解释为模型分析成功。

## 5. Agent 权限边界

| Agent | 运行模式 | 允许动作 | 禁止动作 |
| --- | --- | --- | --- |
| `triage` | 模型只读 | 分析 Issue、建议标签和更新 managed comment | shell、Secrets、代码写入 |
| `planner` | 模型只读 | 拆分任务、验收标准和测试计划 | shell、代码写入 |
| `reviewer` | 模型只读 | 读取仓库和 PR diff、在 managed comment 中发布 advisory 结论 | Check Run、修改代码、批准、合并 |
| `candidate` | 隔离生成 | 在临时 runner 生成有界 patch artifact | GitHub 写入、Writer/release secret、候选外 shell |
| `conflict-resolver` | 模型只读 | 仅从 `agent` Environment 取得模型 secret，为受限上游文本冲突生成完整内容的候选 artifact | GitHub 写入、Writer/release secret、直接合并 |
| `conflict-reviewer` | 不同 model ID 的模型只读 | 仅从 `agent` Environment 取得模型 secret，审查 Resolver artifact；不得与 Resolver 使用相同 model ID | GitHub 写入、修改候选、批准、合并 |
| `publisher` | 单 Writer App | 写 `agent/**` 分支、创建或复用 Draft PR | `.github/**`、审批、Checks、直接合并 |
| `tester` | 确定性 | 触发和读取 CI | 修改源码、合并 |
| `security` | 模型只读 | 审查敏感路径和依赖元数据 | 修改代码、合并 |
| `policy` | GitHub Actions | 以 base 受信代码计算 `Automation Policy / gate` | 使用 LLM 决策、修改代码、合并 |
| `finalizer` | GitHub Actions + Writer token | 复核精确状态后调用一次 `mergePullRequest(SQUASH, expectedHeadOid)` | checkout PR head、native auto-merge、绕过 Policy/保护 |

多个逻辑 Agent 运行在 GitHub Actions 中。开放代码写入和 direct merge 时，仅 Publisher/Finalizer 临时铸造同一个 Writer App installation token；Policy 与 Finalizer 的普通 PR/CI 读侧复核使用 `GITHUB_TOKEN`。Finalizer 在 token mint 前只执行不读取 branch protection/ruleset 的 preliminary proof，该结果只允许进入 mint；完整治理证明必须在 mint 后使用同一 Writer token 的 `administration:read` 重新计算。Draft 转 Ready 前和 Ready 后均须完成 full proof，才可执行一次 Writer direct merge mutation。Writer App 不具备 Administration 写权限，Finalizer 不启用 native auto-merge。

## 6. 事件和命令

初期命令使用：

```text
/agent triage
/agent plan
/agent review
/agent status
/agent cancel
```

真实 App 上线后可使用 `@aeris-agent <command>`。当前 Actions 阶段只接受整段评论恰好等于一个支持的 `/agent <command>`；正常开关开启时运行时匹配不区分命令大小写并去除首尾空白，紧急关闭状态下 workflow 只旁路常见的小写、首字母大写或全大写 `status/cancel` 写法。Bot managed comment、未知命令和未授权 actor 必须忽略。引用、代码块或带附加说明的评论不会被解析为命令。

OWNER、MEMBER 和 COLLABORATOR 创建的 Issue 可进入自动只读分析；其他作者的 Issue 先保持 `status:triage`，只有维护者添加 `agent-analyze` 后才可调用模型。该限制在公开仓库中用于控制提示注入和额度滥用。

事件处理顺序：

1. 读取 GitHub event 和 delivery/source key；不能假设事件只投递一次。
2. 从受保护的 `main@SHA` 加载 registry、policy 和运行器。
3. 检查 kill switch、actor、命令和 best-effort 限额。
4. 构造实际发送给模型的有界规范化输入，并计算 SHA-256 `input_sha`；该 fingerprint 是对象的 canonical generation。
5. 同时记录 GitHub 快速 generation：Issue 的 `updated_at` 或 PR 的 head SHA；它只是提示性的快速筛选，不是权威 generation，最终判定必须重算 `input_sha`。
6. `reserve` job 通过 managed comment 尝试预约本次运行，并写入随机 `lease_token` 与 `cancel_epoch`；`publish` 用它们拒绝已取消或失去预约的旧结果。这只是应用层 fencing 和 best-effort 预约，不是 GitHub 提供的原子锁或 CAS。
7. `analyze` job 重新读取输入并验证 `input_sha`，再调用单个只读 Agent；它校验结构化输出后生成有界、无 Secret 的 JSON artifact，原始输入和 patch 不进入 artifact。
8. `publish` job 不携带 AI Key；它验证 artifact 和受信策略，重新获取对象并重算输入 fingerprint。
9. GitHub 快速 generation 变化时重新获取对象；重算的 fingerprint 不匹配则废弃结果，匹配时确定性更新 managed comment。

Agent handoff 只能选择 registry 中允许的目标，并受最大 handoff、fix cycle 和对象并发数限制。模型不能自行扩大 handoff 图。

## 7. 上游同步

当前同步 workflow 继续保持固定分支、单一开放 PR、人工关闭暂停、显式恢复、`force-with-lease` 和未知 tip 拒绝。Sync 不设置 GitHub native auto-merge：在有变更的本轮中，先等待受保护分支所要求的检查在该精确 head 上成功，再以该 head SHA 调用一次 `PUT /repos/{owner}/{repo}/pulls/{number}/merge`，固定 `merge_method=squash`。这是一笔一次性 mutation，不是可被未来检查、review 或其他状态变化触发的持久授权。

该门禁是有界等待的 required-check success gate；超时、检查未成功、mutation 失败或响应不确定都不重试 merge mutation。每种结果最多做一次独立 PR 回读；只有回读能证明同一 PR 已以 Writer App bot 合并、head SHA 和 base 一致、`auto_merge=null`，且 merge commit 是以当时 base 为唯一 parent 的 squash commit，才算成功。无法证明时 fail closed，保留固定分支和开放 managed PR，供后续同步在身份、tip 和状态仍受信时复用，而不是创建新的 PR 或留下 auto-merge 授权。

同步 workflow 使用 checkpoint 模型，不再依赖 squash 后无法前进的 Git merge-base：

```text
U0 = main 中记录的 last_integrated_sha
M  = 当前受保护 main
U1 = 最新 upstream/main
result = three_way_merge(base=U0, ours=M, theirs=U1)
```

同步状态文件为 `.github/upstream-sync-state.json`，当前 checkpoint 是已由最近一次同步 PR（PR `#16`，经人工三方解决冲突）纳入的 `b7fca851b8c8c357d17d664433f061efaa37b0c9`。该 SHA 位于上游历史；由于本仓库采用 squash-only 合并，它不在 fork `main` 的祖先链中，这是 checkpoint 模型的预期状态而非异常。状态和策略始终从当前受保护的 `main@SHA` 读取；checkpoint 只写入候选结果树，因此必须与同步 PR 一起合并后才会在下一轮生效。每次运行验证 schema、策略版本、上游仓库、分支以及 `U0` 是 `U1` 的祖先。上游历史重写、未知 checkpoint、状态篡改或不受支持的 fork-owned 规则均 fail closed。

路径分类按下列优先级执行：

```text
sensitive > review_required > fork_owned > generated > upstream_owned > default
```

准备合并树时，先将上游的 fork-owned 路径还原为 `U0` 版本，再执行 `base=U0, ours=M, theirs=filtered(U1)` 的三方合并；这样 fork 在 `M` 中的新增、修改和删除均被保留，fork-owned 冲突也不会阻断其他上游增量。当前执行器只支持 `aeris-glob-v1` 的精确路径、目录末尾 `/**`、单段 `*`/`?` 和无斜线 basename 模式；negation、rooted pattern、backslash、character class、空 pattern、尾 `/` 和其他语法均拒绝，不静默猜测。

默认分类是 `review_required`，用于标识同步后的审查风险，不会让未知路径被误认为 `upstream_owned`。`auth`、`migrations`、`security` 路径只要求人工审查；真正禁止发布的 sensitive 集合仅包括 `.gitmodules` 和私钥/证书扩展名。managed 上游同步是通用 Agent Finalizer 之外的确定性例外：fork-owned 路径先被过滤，候选树必须通过 checkpoint、来源、固定分支和精确 head 验证；review-required 或 unknown verdict 可发布人工 PR，但 `autonomous_eligible=false`，不会 direct merge。仅当每个非 fork-owned 冲突均为 UTF-8 `100644` modify/modify 文本时，无 GitHub 写凭据的 AI Resolver 才可产生完整内容的实际 candidate artifact；不同 model ID 的 AI Reviewer 必须独立审查。trusted deterministic verifier 重新物化候选并产生最终 attestation，精确绑定 bundle/candidate/review artifacts、当前 head/tree、base、checkpoint、upstream SHA 与受信 policy；merge helper 还会重新证明严格 required checks、admin-enforced branch protection、零 bypass 和无 active branch ruleset。只有这些绑定与治理条件均成立，Writer 才可执行一次 exact-head REST server-side squash。上游 workflow drift 仍生成或更新审查 Issue，且不会被同步候选覆盖。新增/删除、模式或编码不符、二进制、敏感路径、Reviewer 不独立、artifact/attestation 或任一绑定漂移，均 disarm 历史遗留的 native auto-merge（若存在）并 fail closed；维护者应通过普通 PR 完成人工三方解决，并在同一 PR 中把 `last_integrated_sha` 更新为已实际纳入的上游 SHA。该 PR 合并后，下一轮从新 checkpoint 继续，不再重复旧冲突。

## 8. 幂等、限流和审计边界

运行身份至少由以下字段组成：

```text
source_key (derived replay identity, not a GitHub delivery GUID)
object_id
input_sha (canonical object generation)
object_generation (GitHub fast-check snapshot)
policy_sha
agent
```

每个 Issue/PR 尽量只维护一条带 marker 的 managed comment。`analyze` 和 `publish` 之间的 artifact 是短期、最小化的数据传递载体，不是数据库或审计日志；它必须有大小上限、固定 schema，且不得携带 Secret。若合法的结构化模型输出超过 managed comment 的字符预算，发布器会确定性退化为保留 verdict/risk 和截断摘要的紧凑投影。Actions 日志提供当前阶段的排障线索，但也不构成持久、不可变、可查询的审计库。

managed comment 的“读取后更新”不是 GitHub 提供的原子 compare-and-swap。即使 reservation 写后重读自己的 token，两个并发运行仍可能先后各自观察到自己写入的值并都进入模型调用。Actions `concurrency`、写回前 fingerprint 复核、Bot marker 和运行身份元数据只能减少重复并使重试趋于同一投影；并发、手工编辑、评论删除或 API 超时仍可能产生重复模型调用、重复或覆盖写入以及无法判断的状态。因此 Phase 2.1 不承诺 at-most-once 或 strict exactly-once，也不能把 managed comment 当作锁、租约、CAS 或权威运行状态。重放 ledger 在内存中最多保留最近 32 条；写入 managed comment 时会先裁剪旧原因码，再按最终编码后的评论预算丢弃最旧 ledger 记录，因此这同样只是有界、best-effort 的近期重放抑制。

每对象每小时运行数和并发数同样是 Actions-only 的 best-effort 防滥用护栏，不是精确配额或计费限流。小时窗口按 `[now - 60 minutes, now]` 计算，恰好位于下界的预约仍计入。Bot 自己发布的评论不得再次触发 Agent；发布器在重试前必须重新读取当前状态并尽量收敛。当前只读阶段不创建 PR、写业务标签或执行合并，所以这些高风险 effect 的强幂等问题留在相应阶段启用前解决。

## 9. 自动合并

`Automation Policy / gate` 是 GitHub Actions 的确定性检查。对同步 PR，它只证明 required-check health，不是 upstream policy eligibility attestation；direct merge 的 eligibility 必须来自受信 prepare 输出、精确 commit trailer 和 merge helper 的再次校验。生产 required check 和 production flags 在 PoC 证据完成前均不得启用；PoC 后的推进顺序是：

1. `shadow`：只报告本应允许或拒绝的原因。
2. `human`：维护者参考 Policy 后手工合并。
3. `label`：维护者添加 `automerge-approved` 后允许 Finalizer 在完整 proof 后执行一次 direct squash merge。
4. `allowlist`：仅对经过历史验证的低风险范围自动合并。

所有模式都必须绑定精确 head SHA、最新 base、必需 CI 和讨论解决状态。Finalizer 在 Draft 转 Ready 前和转 Ready 后各运行一次 full proof；proof 同时覆盖 Writer App、installation、单仓 scope、Bot 身份和 `administration:read` 治理读取。第二次 proof 后才调用 `mergePullRequest`，固定 `SQUASH + expectedHeadOid`。mutation 失败不遗留持久授权；响应不确定时停止重试并独立回读合并状态，不能确认即 fail closed。对受限上游冲突，模型自报、评论或未绑定审查结论不能替代 trusted verifier 的 artifact/head/tree/base/checkpoint/upstream/policy attestation。`.github/**`、依赖文件、认证、安全、数据库、发布和其他策略标记路径默认需要人工审查。managed 上游同步 PR 是独立的确定性例外：它使用有界 required-check gate 加一次 exact-head REST squash merge 和严格回读，而不保留 native auto-merge。模型的自报置信度不能改变门禁。

## 10. 威胁模型

首批实现必须覆盖：

- 公开 Issue 或评论造成的 prompt injection 和额度滥用。
- PR 修改 workflow、registry 或 policy 后尝试提升自身权限。
- Writer App 凭据泄漏，或 Policy/Finalizer workflow 被篡改。
- `pull_request_target` 或 `workflow_run` 误用导致不可信代码接触 Secrets。
- webhook 重放、乱序、重复 delivery 和对象更新竞态。
- artifact 超大、schema 被替换、跨 job 夹带 Secret 或复用到错误对象。
- 模型输出超大、非 JSON、未知 Agent、任意模型名或工具参数。
- 上游 force-push、checkpoint 回退和同步分支未知提交。

模型阶段不得获得 GitHub 写凭据、Writer App 私钥或发布凭据。Candidate 在临时隔离环境中运行；`agent` Environment 无人工审批但不能访问 Writer 或 release secret。Candidate 的受信 extractor 由独立 job 从精确 base SHA 封装，必须在模型结束后下载，并通过隔离 Git directory/index 和空配置读取工作树，不能信任 Agent 可写的 runtime、`.git/config`、hooks、filters、textconv 或 fsmonitor。Policy 是 Actions job；Finalizer 的唯一写入是在第二次 full proof 后，使用临时 Writer token 调用一次 `mergePullRequest(SQUASH, expectedHeadOid)`。它不得启用 native auto-merge，`release` Environment 仍需人工审批。

## 11. 实施顺序

1. Phase 0：策略契约、威胁模型、kill switch 和仓库设置审计（已实现，默认关闭）。
2. Phase 1：checkpoint 同步 PoC、测试和迁移（已实现；首次冲突路径已经 PR `#16` 人工三方解决并实证，checkpoint 已追平 `upstream/main`，后续回到定时 no-op/自动 PR 循环）。
3. Phase 2.1：Actions-only 的只读 triage/planner/reviewer；模型分析与确定性写回分 job，通过有界无 Secret 的 JSON artifact 交接。
4. Phase 3：Candidate artifact、单 Writer App Publisher 和 Draft PR；先完成 artifact、凭据隔离和事件语义 PoC。
5. Phase 4：GitHub Actions Policy 的 shadow/human gate；仅在来源和漂移 PoC 完成后加入 required check。
6. Phase 5：Finalizer 仅对 `docs/automation-canary/**/*.md` 在双重 full proof 后执行一次 direct squash merge；production flags 仅在全部 PoC、撤销演练和稳定观察后启用。
7. Phase 6：仅在真实运行证明 Actions-only 无法满足要求后，才引入外部状态或工作流服务。触发条件包括多 worker/多实例协调、可靠跨运行恢复或复杂 DAG、精确额度与计费限流、需要事务化 outbox/effect receipt，或不可变且可查询的审计要求。

Phase 6 需要的是符合一致性和运维要求的权威状态层，不等于必须使用 Postgres。单实例可评估 SQLite；云原生环境可评估具备条件写入的托管 KV、Durable Objects、Kubernetes CRD 或工作流引擎；需要关系查询、多 worker 事务和 outbox 时 Postgres 通常更合适。选型必须由已观测的并发、恢复、审计和运维需求驱动，不能因为规划中的功能提前引入数据库。

每个 Phase 必须独立提交、独立验证并可回滚。Phase 0 和 Phase 1 未通过前，不开放模型代码写入；Policy shadow 数据未完成复核前，不开放自动合并。

## 12. 仓库设置清单

下列设置不由策略文件自动生效，需要维护者在 GitHub Settings 中单独确认：

- Actions 默认令牌保持只读。
- 仅在同步工作流需要时允许 GitHub Actions 创建 PR；不得以该设置绕过审批或分支保护。
- 限制允许的 Actions 来源，关键写权限 Action 固定完整 SHA。
- `Rust CI / check`、`Frontend CI / check` 和 `Automation Policy / gate` 是 `main` 唯一的三项 strict required checks，并绑定 GitHub Actions source；不新增 Finalizer hold 的第四 required context。
- `agent` Environment 不得设置人工审批，且仅向 Candidate 暴露模型凭据；`writer` Environment 才保存 Writer App 凭据。
- `release` Environment 必须保留人工审批；不得由 Agent 或 Writer App 使用。
- 通用 Agent 自动合并、Policy required check 和 production flags 在对应 PoC 完成前不得启用；managed 上游同步的一次性 REST merge 路径也须单独审计，不能因 Finalizer 通过 GraphQL direct-merge PoC 而自动获准。

任何远端设置变更都应记录在 Issue `#11`，并通过当前配置的现场读取结果验证。
