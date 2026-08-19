# GitHub 自动化与 Agent 架构

本文定义 `aeris-token` 的上游同步、Issue 分析、Agent 协作和条件自动合并边界。它是实施契约，不代表所有能力已经启用。各阶段只有在对应 workflow、测试和仓库设置完成后才生效。

实施总入口为 GitHub Issue `#11`。日常开发流程仍以 [development-workflow.md](development-workflow.md) 为准。

## 1. 设计原则

1. `main` 继续保持 squash-only 和线性历史。
2. 自动化只通过分支和 Pull Request 修改代码，不直接写入 `main`。
3. Issue、评论、PR 内容和模型输出均是不可信输入。
4. 模型负责分析和建议；授权、路径限制和合并由确定性规则决定。
5. Writer、Policy 和 Merger 是不同权限边界，不能由同一令牌完成全部动作。
6. 所有策略从受保护的 `main@SHA` 读取，不能从当前 PR checkout 读取。
7. GitHub 事件按可能重复、乱序和重试处理；Actions-only 阶段只提供 best-effort 去重和收敛，不声明 strict exactly-once。
8. 功能按只读分析、Draft PR、Policy Gate、自动合并的顺序逐级开放。

## 2. 当前阶段

仓库内的策略源为：

- `.github/agents.yml`：Agent registry、模型路由和逻辑能力。
- `.github/automation-policy.yml`：授权、限额、幂等、安全和合并门禁。
- `.github/upstream-sync-policy.yml`：上游身份、路径所有权和 checkpoint 约束。

这些文件当前默认关闭所有新 Agent：

```text
AERIS_AGENTS_ENABLED != true
```

Phase 2.1 采用 Actions-only，不部署常驻 Coordinator 或外部数据库。每条只读 workflow 分成四个权限隔离的 job：`preflight` 读取受信配置并验证调用，`reserve` 对 managed comment 做 best-effort 预约，`analyze` 以只读 GitHub 权限调用模型，`publish` 确定性写回。只有真正进入 reserved 分支的 `analyze` step 接收 AI Key；terminal passthrough step 不接收 Secret。阶段间只传递经过 schema 校验的有界 JSON artifact，运行器不会把输入正文或 PR patch 直接复制进 artifact；`analyze` 重新读取输入并核对 fingerprint，`publish` 再次获取目标输入并复核 fingerprint 后更新 managed comment。artifact 不得包含 Secret、Authorization、Cookie 或请求头，并受 registry 中的字节上限约束；模型输出还会在 artifact 写入前拒绝当前 AI Key 和常见认证头/Bearer 形态，但这属于针对已知运行 Secret 的防泄漏护栏，不是通用 DLP。结构化模型输出仍可能概括或引用输入内容，因此 artifact 按目标仓库数据同级处理。`reserve` 不是强租约，artifact 也不是锁或权威状态源。

Phase 2.1 的 workflow 和运行器已合并并通过远端 CI，triage 已完成端到端实证（真实模型分析发布 managed comment，修复链见 PR `#19`/`#20`/`#22`/`#23`）。当前显式启用的 Agent 是 `triage`、`planner` 和 `reviewer`（kill switch 与 registry 双开）；`reviewer` 合并后通过独立的 owner-authored canary PR 完成受控验证。`issue-triage.yml` 继续确定性添加 `status:triage`。这三个 Agent 的唯一 GitHub 写入投影是 managed comment；Reviewer 的 `analyze` job 保持 `pull-requests: read`，无 Secret 的确定性 `reserve`/`publish` job 使用 `pull-requests: write` 写入 PR 普通评论。其受信运行器只调用 Issue Comments 的 POST/PATCH，不包含 review、approve 或 merge API，也不发布 Check Run。由于同步 workflow 需要仓库级 Actions 创建 PR 开关，此 token 权限在能力层面仍覆盖 approval；canary PR `#34` 的 run `32109708799` 已证明仅改用 `issues: write` 会在 PR 评论写回时返回 HTTP 403。后续若要消除该能力残余，必须改用权限独立的 GitHub App，而不是再次缩减 `GITHUB_TOKEN`。Writer、Policy 和 Merger 继续关闭。Phase 2.1 不修改业务标签、代码、审批或合并状态。

Phase 3 当前只提供默认关闭的 Writer 基础契约，不提供可触发的 Writer workflow，也不具备远端写入能力。基础层包含独立开关与 GitHub App 身份声明、仅接受精确完整 Writer 命令的解析器、从现场 GitHub API 重读评论作者权限与 Issue 状态/标签的授权路径、路径校验纯函数、Draft PR 生命周期判定、专用有界 artifact schema，以及不暴露 review/merge API 的受限 GitHub client。registry 与 policy 仍同时要求 `writer.enabled: false`；在独立 App、`writer` Environment、分支保护和真实 canary 全部完成前，任何人都不能仅通过设置变量启用 Writer。尤其 Writer workflow 尚不存在，因而发布前的第二次现场状态、权限、精确 head 和 artifact 绑定复核尚未实现；这是启用前 blocker，不能把当前基础契约表述为写入闭环。

## 3. 信任和配置来源

每次运行必须先取得默认分支的精确 SHA，并通过 GitHub API 从该 SHA 加载 registry 和 policy。以下来源不能影响本次授权：

- Issue 或评论中给出的模型名、路径规则或权限声明。
- 当前 PR 对 `.github/**` 的修改。
- 模型返回的工具参数、shell 命令或后续 Agent 名称。
- 未经 registry 允许的 webhook actor、命令或 handoff。

策略文件、同步状态、CODEOWNERS、workflows 和 Git 元数据（`**/.git`、`**/.git/**`）是不可由 Writer 修改的控制面文件。涉及这些文件的变更必须由维护者审查。

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
| `writer` | 隔离写入 | 写 `agent/**` 分支、创建 Draft PR | `.github/**`、审批、Checks、合并 |
| `tester` | 确定性 | 触发和读取 CI | 修改源码、合并 |
| `security` | 模型只读 | 审查敏感路径和依赖元数据 | 修改代码、合并 |
| `policy` | 确定性 | 发布 `Automation Policy / gate` | 使用 LLM 决策、修改代码、合并 |
| `merger` | 确定性 | 合并精确已验证 head SHA | 修改分支内容、绕过 Policy |

多个逻辑 Agent 可以先运行在 GitHub Actions 中。只有开放代码写入和自动合并时，才需要独立 GitHub App 身份实现权限隔离。

Writer 使用专用私有 GitHub App，安装范围仅限本仓库，声明权限固定为 `Metadata: read`、`Contents: write` 和 `Pull requests: write`。App ID 与 slug 分别由 `AERIS_WRITER_APP_ID`、`AERIS_WRITER_APP_SLUG` Variable 提供，私钥仅存放在受保护的 `writer` Environment Secret `AERIS_WRITER_PRIVATE_KEY` 中。未来确定性 publish job 必须从 App ID/私钥生成短期 App JWT；客户端只用该 JWT 调用 `/app` 核对 ID/slug、读取目标仓库 installation 并核对 owner/权限，再由该 JWT 为现场 installation mint 临时 token。新 token 只用于 `/installation/repositories` 和后续受限 Writer API，并须核对 `selected` 模式、唯一仓库 ID、完整仓库名、到期时间和最小权限；调用方不能注入预先生成的 installation token，App JWT 与 minted token 也不能相同或互换。模型生成 job 和测试 job不得读取 App 凭据，确定性 publish job不得读取 AI Key 或执行候选代码。Writer API 调用具有固定的 headers/body/总 deadline，响应按流读取并在 1 MiB 处立即中止；未来 publish job 必须显式设置 15 分钟 `timeout-minutes`。Writer 还必须同时通过全局 `AERIS_AGENTS_ENABLED`、专用 `AERIS_WRITER_ENABLED` 和受信 registry/policy 三重门禁。

GitHub App 权限粒度不能把 `Contents: write` 限定到 `agent/**`，`Pull requests: write` 也不能在 IAM 层排除 review 或 merge。因此独立 App 是身份隔离，不是完整 capability boundary。剩余能力通过无 branch-protection bypass、受限 client、禁止 review/approve/merge/auto-merge/mark-ready/通用 close/delete 操作、精确 head fencing 和人工审批缓解。唯一的 close 例外是创建 Draft PR 发生竞态或响应不确定后的补偿：每次 create 在 body 中加入由 `crypto.randomUUID()` 生成的隐藏 attempt marker；`POST /pulls` 发生 headers/total timeout、连接异常或 `201` body timeout、截断、超限、无效 JSON 时绝不重试 POST，而是按精确 head 枚举现场 PR。只有恰好一个 PR 包含本次 attempt marker，且 Writer App 身份、draft/未合并状态、仓库、base/head、SHA、标题、完整 body 和 canonical ownership marker 全部匹配时才允许关闭并复读确认；零个、多个、字段漂移或复读失败均 fail closed 并报告可能存在平台残留。若要求凭据本身不具备这些能力，则必须引入外部 capability broker，不能把 Actions-only 方案描述为已经满足。

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

当前同步 workflow 继续保持固定分支、单一开放 PR、人工关闭暂停、显式恢复、`force-with-lease` 和未知 tip 拒绝。干净生成的 managed 同步 PR 使用 GitHub 原生 squash auto-merge；分支保护负责等待精确 head 的必需 CI、最新 `main`、讨论解决和无冲突状态。

同步 workflow 使用 checkpoint 模型，不再依赖 squash 后无法前进的 Git merge-base：

```text
U0 = main 中记录的 last_integrated_sha
M  = 当前受保护 main
U1 = 最新 upstream/main
result = three_way_merge(base=U0, ours=M, theirs=U1)
```

同步状态文件为 `.github/upstream-sync-state.json`，当前 checkpoint 是同步 PR `#33` 纳入的 `535ee098c35959344db4b1186dc09a858912469e`；随后 run `32105256165` 已验证同一上游 head 会稳定 no-op。该 SHA 位于上游历史；由于本仓库采用 squash-only 合并，它不在 fork `main` 的祖先链中，这是 checkpoint 模型的预期状态而非异常。状态和策略始终从当前受保护的 `main@SHA` 读取；checkpoint 只写入候选结果树，因此必须与同步 PR 一起合并后才会在下一轮生效。每次运行验证 schema、策略版本、上游仓库、分支以及 `U0` 是 `U1` 的祖先。上游历史重写、未知 checkpoint、状态篡改或不受支持的 fork-owned 规则均 fail closed。

路径分类按下列优先级执行：

```text
fork_owned > review_required > generated > upstream_owned > default
```

准备合并树时，先将上游的 fork-owned 路径还原为 `U0` 版本，再执行 `base=U0, ours=M, theirs=filtered(U1)` 的三方合并；这样 fork 在 `M` 中的新增、修改和删除均被保留，fork-owned 冲突也不会阻断其他上游增量。当前执行器对 fork-owned 支持精确路径和目录末尾 `/**`；策略出现其他 glob 时拒绝运行，避免静默误分类。

默认分类是 `review_required`，用于标识同步后的审查风险，不会让未知路径被误认为 `upstream_owned`。managed 上游同步是通用 Agent Merger 之外的确定性例外：fork-owned 路径先被过滤，候选树必须通过 checkpoint、来源、固定分支和精确 head 验证，随后仅由分支保护决定原生 auto-merge。上游 workflow drift 仍生成或更新审查 Issue，且不会被同步候选覆盖。非 fork-owned 冲突时，自动化会先撤销旧 auto-merge，不生成伪解决方案或覆盖未知同步分支；维护者应通过普通 PR 完成人工三方解决，并在同一 PR 中把 `last_integrated_sha` 更新为已实际纳入的上游 SHA。该 PR 合并后，下一轮从新 checkpoint 继续，不再重复旧冲突。

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

Writer 的 Draft PR 元数据更新和 agent ref 推进同样没有 REST 级别的 compare-and-swap：每次 mutation 前会连续完整重读并验证既有 PR/ref fencing，写后立即读回并要求精确的预期 head SHA；任何漂移都会停止后续 mutation 并 fail closed。POST 建 PR 的不确定结果则始终有界枚举同一 head 的所有 open/closed PR，要求本次 attempt marker 全局唯一，并在关闭前再次读取、验证 App 所有者、draft/open、base、head SHA 和完整 marker/body。这个流程缩窄竞态窗口并使未确认结果可见，但不能把 REST PATCH 描述为原子操作；最后一次读和写之间仍可能发生并发变更，读回不符时会报告残留而不继续写入。

每对象每小时运行数和并发数同样是 Actions-only 的 best-effort 防滥用护栏，不是精确配额或计费限流。小时窗口按 `[now - 60 minutes, now]` 计算，恰好位于下界的预约仍计入。Bot 自己发布的评论不得再次触发 Agent；发布器在重试前必须重新读取当前状态并尽量收敛。当前只读阶段不创建 PR、写业务标签或执行合并，所以这些高风险 effect 的强幂等问题留在相应阶段启用前解决。

## 9. 自动合并

`Automation Policy / gate` 是确定性检查，初始模式固定为 `shadow`。推进顺序是：

1. `shadow`：只报告本应允许或拒绝的原因。
2. `human`：维护者参考 Policy 后手工合并。
3. `label`：维护者添加 `automerge-approved` 后允许 Merger 执行。
4. `allowlist`：仅对经过历史验证的低风险范围自动合并。

所有模式都必须绑定精确 head SHA、最新 base、必需 CI 和讨论解决状态。`.github/**`、依赖文件、认证、安全、数据库、发布和其他策略标记路径默认需要人工审查。managed 上游同步 PR 是独立的确定性例外：它不使用模型或通用 Merger，只在 checkpoint 合并无冲突、fork-owned 路径已过滤且严格分支保护全部满足时执行原生 squash auto-merge。模型的自报置信度不能改变门禁。

## 10. 威胁模型

首批实现必须覆盖：

- 公开 Issue 或评论造成的 prompt injection 和额度滥用。
- PR 修改 workflow、registry 或 policy 后尝试提升自身权限。
- Writer、Policy 或 Merger 凭据泄漏。
- `pull_request_target` 或 `workflow_run` 误用导致不可信代码接触 Secrets。
- webhook 重放、乱序、重复 delivery 和对象更新竞态。
- artifact 超大、schema 被替换、跨 job 夹带 Secret 或复用到错误对象。
- 模型输出超大、非 JSON、未知 Agent、任意模型名或工具参数。
- 上游 force-push、checkpoint 回退和同步分支未知提交。

Phase 2.1 的模型阶段不得获得 shell 或任意网络工具。未来 Writer 的模型执行必须位于临时隔离环境中，且不能访问 Writer App、发布 Environment 或其他写入 Secret。最终 publish job 只能消费严格校验并绑定精确 base/policy/Issue generation 的候选 artifact，不能执行候选代码。Policy 和 Merger 不能与 Writer 共用可生成同等权限令牌的凭据。

## 11. 实施顺序

1. Phase 0：策略契约、威胁模型、kill switch 和仓库设置审计（已实现，默认关闭）。
2. Phase 1：checkpoint 同步 PoC、测试和迁移（已实现；首次冲突路径已经 PR `#16` 人工三方解决并实证，checkpoint 已追平 `upstream/main`，后续回到定时 no-op/自动 PR 循环）。
3. Phase 2.1：Actions-only 的只读 triage/planner/reviewer；模型分析与确定性写回分 job，通过有界无 Secret 的 JSON artifact 交接（已合并并逐个完成远端验证；Reviewer canary 证据记录于 Issue `#11`）。
4. Phase 3：独立 Writer 身份和 Draft PR（当前仅有默认关闭的基础契约；workflow、App/Environment、受控生成器、publish 执行器和 canary 均未启用）。
5. Phase 4：独立 Policy 身份及 shadow/human gate。
6. Phase 5：独立 Merger 身份及极小 allowlist。
7. Phase 6：仅在真实运行证明 Actions-only 无法满足要求后，才引入外部状态或工作流服务。触发条件包括多 worker/多实例协调、可靠跨运行恢复或复杂 DAG、精确额度与计费限流、需要事务化 outbox/effect receipt，或不可变且可查询的审计要求。

Phase 6 需要的是符合一致性和运维要求的权威状态层，不等于必须使用 Postgres。单实例可评估 SQLite；云原生环境可评估具备条件写入的托管 KV、Durable Objects、Kubernetes CRD 或工作流引擎；需要关系查询、多 worker 事务和 outbox 时 Postgres 通常更合适。选型必须由已观测的并发、恢复、审计和运维需求驱动，不能因为规划中的功能提前引入数据库。

每个 Phase 必须独立提交、独立验证并可回滚。Phase 0 和 Phase 1 未通过前，不开放模型代码写入；Policy shadow 数据未完成复核前，不开放自动合并。

## 12. 仓库设置清单

下列设置不由策略文件自动生效，需要维护者在 GitHub Settings 中单独确认：

- Actions 默认令牌保持只读。
- 仅在同步工作流需要时允许 GitHub Actions 创建 PR；不得以该设置绕过审批或分支保护。
- 限制允许的 Actions 来源，关键写权限 Action 固定完整 SHA。
- `Rust CI / check` 和 `Frontend CI / check` 保持 strict required checks。
- 通用 Agent 自动合并在 Policy Gate 进入要求检查前不得启用；managed 上游同步仅使用上述确定性原生 auto-merge 例外。
- `writer`、`policy`、`merger` 和 `sync` Agent Environment 仅允许 `main`，不要求人工 reviewer，且管理员不能绕过 Environment 保护。
- 发布 Secrets 仅保存在受保护的 `release` Environment；该 Environment 继续要求 `JinPengGeng` 人工审批，且管理员不能绕过。
- Writer 启用前必须已有独立 `writer` Environment 和仓库级私有 GitHub App；App 不得拥有 branch-protection bypass。Writer workflow、受控生成器、发布前二次现场复核和真实 canary 仍是独立 activation blocker。

任何远端设置变更都应记录在 Issue `#11`，并通过当前配置的现场读取结果验证。
