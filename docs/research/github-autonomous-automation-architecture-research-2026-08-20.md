# GitHub 自主开发与上游同步架构调研

> 日期：2026-08-20
> 状态：`research-done / implementation-in-progress / acceptance-pending`
> 决策前提：四 GitHub App 方案废弃
> 适用对象：单个公开 GitHub fork，周期性同步上游，AI 可提交代码 PR，低风险变更在确定性门禁通过后可无人值守合并，冲突和语义不明时 fail closed，release 保持人工审批
> 边界：本报告是架构依据；实际完成状态以实施规格、自动化测试和 GitHub 现场验收证据为准，不恢复旧四 App 方案

> **后续决策（2026-08-21，取代本报告中关于 terminal hold 与 persistent GitHub native auto-merge 的实施建议）：** 为消除阻塞条件在未来解除后、未重新执行 Finalizer 即合并的陈旧授权风险，生产实现改为一次性的服务端 GraphQL `mergePullRequest`：在 Draft 转 Ready 前及 Ready 后完成完整证明后，以固定 `SQUASH` 和 `expectedHeadOid` 直接请求合并。失败不保留持久 auto-merge 授权，响应不确定时必须回读 PR 结果；其余研究历史、证据和方案比较仍保留供追溯。

## 1. 结论先行

### 1.1 推荐结论

#### 后续实施决策

本节原推荐的 GitHub native auto-merge 已不再作为生产合并执行方式。当前实施保留 GitHub 保护规则、最小 Writer App、Draft PR、CI 和确定性 Policy 门禁，但由 Finalizer 在两次完整证明均通过后调用一次性 GraphQL direct squash merge；任何错误、head/governance 漂移或结果不确定都 fail closed，且不得留下持久的 auto-merge 请求。

四 App 方案属于过度设计。当前目标不需要四个长期高权限身份，也不需要自建合并器。

推荐采用：

**GitHub 原生治理 + 一个最小 Writer App + 可替换的 AI 执行器 + GitHub native auto-merge。**

```text
可信触发器
  -> AI 执行环境（仅持有模型访问凭据；无 GitHub 写凭据、无 Writer/release secret、受限网络）
  -> 候选 patch / commit
  -> 确定性 Publisher（短期 Writer App token）
  -> 固定命名空间分支 + Draft PR
  -> PR CI（无 secrets）
  -> 确定性 Policy job（required check）
  -> GitHub ruleset / branch protection
  -> GitHub native auto-merge
  -> protected main

周期性 upstream sync
  -> 同一个 Publisher/Writer 身份
  -> 固定同步分支 + 复用同一个开放 PR
  -> 同一套 CI / Policy / auto-merge

release
  -> 独立 GitHub Environment
  -> 人工审批
```

核心取舍：

1. **Policy 不是 App。** Policy 应是版本化、确定性的 required check，由 ruleset/branch protection 强制执行。
2. **Merger 不是 App。** 自动化只请求启用 auto-merge；最终合并由 GitHub 在实时条件满足时完成。
3. **Sync 默认不是独立 App。** Sync 与 Writer 在单仓库内都只需要 `contents:write` 和 `pull_requests:write`，拆分身份并不会获得分支级权限隔离。
4. **Writer App 是实现无人值守 PR 事件链的最小例外。** 纯 `GITHUB_TOKEN` 创建或更新 PR 时，GitHub 会抑制大多数由该 token 触发的后续 workflow 事件，而不是把它们统一置为 approval-required。若要求 PR CI 全无人值守，一个最小 Writer App 比复制 CI、构造自定义调度链更简单。
5. **AI 不能是合并裁决者。** AI 生成 patch、解释风险或提出建议；可自动合并的资格只能由确定性规则、CI、可信 check 来源和 GitHub 保护规则决定。
6. **个人 fork 当前不应自建 merge queue。** 低并发采用 strict required checks；未来迁入组织且合并并发明显上升时，再启用 GitHub Merge Queue。

### 1.2 四 App 如何被替代

| 原角色 | 替代机制 | 为什么更简单 |
|---|---|---|
| Writer App | 保留一个最小 Writer App | 仅负责受控分支和 PR 写入，不参与策略裁决或绕过保护直接合并 |
| Policy App | GitHub Actions required policy job + expected source | 策略与代码同版本，结果绑定 commit，GitHub 负责强制 |
| Merger App | GitHub native auto-merge | 避免自建服务在检查、审批、head/base 漂移之间产生 TOCTOU 竞态 |
| Sync App | 默认复用 Writer App；允许人工审批时可仅用 `GITHUB_TOKEN` | 单仓库权限集合相同，独立 App 不是 API 功能要求 |

单独创建 Sync App 只在以下条件成立时才有安全价值：

- Agent 处理不可信 issue、评论或模型输入，而 Sync 处理受信上游，且需要硬隔离根凭据。
- 两类任务覆盖不同仓库、不同所有者、不同轮换和撤销周期。
- 其中一个身份需要 ruleset bypass、跨仓库访问或私有上游凭据。
- 合规要求独立审计身份和职责分离。

否则，两个 App 只是增加私钥、安装、轮换、告警和恢复成本。GitHub App 的 `contents:write` 不能原生限制到 `agent/**` 或 `automation/**` 分支，因此仅拆 App 也不等于 capability 隔离。

## 2. 决策问题与研究边界

### 2.1 决策问题

需要找到一套比四 App 更简单、同时满足以下结果的架构：

- 定时追踪上游，正常情况下复用同一同步分支和 PR。
- AI 能异步处理允许范围内的任务并形成可审查 PR。
- 无 Git 冲突、required CI 全绿、确定性策略满足时，可无人值守 squash merge。
- Git 冲突、敏感路径、语义不确定、来源异常、凭据异常、GitHub 故障或持续 CI 失败时停止。
- release 与生产相关操作继续人工审批。
- 不因身份拆分而自行实现 GitHub 已经提供的保护、检查和合并功能。

### 2.2 非目标

- 不选择或实施具体模型。
- 不评审现有 workflow、脚本或未提交工作树。
- 不创建 App、secret、environment、ruleset、PR 或 commit。
- 不保证任一托管 Agent 产品满足本项目全部需求；产品能力仍需 PoC 验证。
- 不把 GitHub stars、供应商宣传或社区讨论当作安全与正确性证明。

## 3. 方法、规模与证据等级

### 3.1 搜索与筛选

本轮保留了四个 GitHub 检索候选池：

| 候选池 | 原始结果条数 | 用途 |
|---|---:|---|
| merge / PR automation | 190 | 发现合并队列、机器人和策略工具 |
| AI coding agents | 163 | 发现托管及开源编码 Agent |
| GitHub automation architecture | 49 | 交叉发现；该轮曾受 Windows 编码异常影响，不用于数量结论 |
| fork / upstream sync | 180 | 发现 fork 同步和仓库镜像模式 |
| 合计 | 582 | 含重复和大量误报，只是 discovery corpus |

筛选规则：

1. 最终事实优先使用官方产品文档、官方仓库、GitHub 实时元数据和 Git/GitHub 官方语义。
2. 开源项目只有在机制与目标直接相关时进入 shortlist；stars 只作发现信号。
3. 已归档、官方弃用或当前文档无法验证的方案不得作为正式推荐。
4. 供应商对自身安全、隔离和成本的描述标记为“官方声明”，不等同于独立审计。
5. 事实、架构推论和本报告建议分开陈述。

### 3.2 证据等级

| 等级 | 定义 | 本报告用法 |
|---|---|---|
| A | 当前 GitHub/API 观察结果 | 当前仓库类型、owner 类型、auto-merge 开关等 |
| B | 官方文档、标准或官方仓库 | 机制、权限、事件、安全边界和产品限制 |
| C | 供应商产品声明或维护者说明 | 托管隔离、定价、路线和运维能力 |
| D | 推论或建议 | 由目标约束与 A/B/C 证据推导，必须经 PoC 验证 |

### 3.3 当前适用性事实

只核验了与方案适用性直接相关的仓库元数据，没有读取实现代码：

- 当前仓库是 `JinPengGeng/aeris-token`，公开、个人账户拥有的 fork，默认分支为 `main`。[R1]
- GitHub repository API 当前显示 `allow_auto_merge=true`。[R1]
- GitHub Merge Queue 的正式适用范围和当前个人 fork 不匹配；迁入组织后需重新核验套餐和规则可用性。[G6]

## 4. 业界共同架构模式

GitHub、GitLab、Azure DevOps、Gerrit、Mergify、Prow 和 Zuul 的实现形式不同，但可靠设计收敛到以下模式。

### 4.1 平台保护规则是最终权威

CI、Agent 和 bot 都只能提供候选结果。是否允许进入目标分支，应由服务端 ruleset、branch protection、required checks、审批要求和当前冲突状态共同决定。

这意味着：

- 不使用模型评论、PR label 或自建 receipt 单独授权合并。
- Writer 身份不进入 bypass 列表。
- required check 绑定可信 expected source，防止其他 App 伪造同名状态；expected source 只能约束发布 App，不能区分同一 App 下的具体 workflow，因此 Policy 还必须由 base 分支的受信 workflow/代码运行，并对治理路径使用人工 lane。
- 自动化只调用“启用 auto-merge”或“加入 merge queue”，不调用绕过保护的直接 merge。

### 4.2 验证对象是候选合并结果，而不只是 PR head

高并发系统会测试 `latest base + candidate PR + queue predecessors` 的合成提交。GitHub Merge Queue、GitLab merge trains、Prow/Tide 和 Zuul 都体现了这一点。[G6][X1][M4][M5]

当前低并发个人 fork 可用 strict required checks 要求 PR 分支与 base 保持最新。未来出现多个并发可合并 PR 时，应迁移到 merge queue，而不是在 Actions 里手写分布式队列。

### 4.3 提议权与合并权分离

Agent/Writer 可以：

- 创建受控分支。
- 推送候选 commit。
- 创建或更新 PR。
- 请求 auto-merge。

Agent/Writer 不应：

- 绕过 ruleset。
- 发布 required policy check。
- 批准自己的 PR。
- 直接更新 `main`。
- 持有 release secret。

### 4.4 执行环境短暂且最小授权

GitHub Copilot cloud agent、Codex Cloud、Cursor Cloud Agents、Devin 和 OpenHands Cloud 都采用某种隔离执行环境，但安全边界并不等价。[A1][A2][A5]

共同的安全做法是：

- 每任务干净环境或一次性 runner。
- Agent 阶段没有长期 GitHub 写凭据。
- secret 只在 setup/publisher 阶段出现，或者改用 OIDC 换取短期外部凭据。
- 默认无网络或仅 allowlist，限制非读取 HTTP 方法。
- Agent 输出按不可信输入处理，再由确定性 Publisher 校验。

### 4.5 事件至少一次，处理必须幂等

Webhook、Actions rerun、网络重试和 runner 中断都会造成重复执行。可靠自动化依靠：

- 固定分支和开放 PR 复用。
- `{repository, task, baseSha, headSha}` 幂等键。
- 写入前重读 head/base/PR 状态。
- 有界重试和明确终态。
- 不把 Actions `concurrency` 当作数据库锁或事务。

### 4.6 fail closed 是状态机，不是 catch-all

应区分：

- `retryable_infrastructure_failure`：短暂网络、GitHub 5xx、runner 暂错，可有界重试。
- `candidate_failure`：编译、测试、lint 或策略失败，不无限自动修复。
- `conflict_or_stale`：base/head 漂移、Git 冲突、审批失效，重新生成候选或转人工。
- `security_boundary_failure`：凭据、来源、签名、权限、敏感路径异常，立即停止并告警。
- `release_or_production`：永远进入人工审批路径。

## 5. 候选架构比较

以下评分是面向本目标的设计判断，不是产品 benchmark。

| 方案 | 身份数量 | 无人值守 PR CI | 最终合并权威 | 运维复杂度 | 当前结论 |
|---|---:|---|---|---|---|
| A. GitHub 全原生，零自建 App | 0 | 有缺口 | GitHub | 最低 | 仅适合允许人工批准 workflow，或愿意显式 dispatch/内嵌 CI 的场景 |
| B. 原生治理 + 一个 Writer App | 1 | 支持 | GitHub | 低 | **推荐** |
| C. Writer + Sync 两 App | 2 | 支持 | GitHub | 中 | 仅在两个信任域必须硬隔离时采用 |
| D. 单 control-plane App + 外部服务 | 1 | 支持 | GitHub或外部控制面 | 高 | 多仓库、凭据 broker、持久状态出现后再评估 |
| E. 托管 coding agent + 原生门禁 | 供应商 App | 视产品而定 | GitHub | 低到中 | 可替换执行器；必须检查其人工 review 硬限制和权限集合 |
| F. SaaS merge/controller | 第三方 App | 支持 | SaaS + GitHub | 中 | 当前收益不足；无法用原生 queue 时的备选 |
| G. 外部 durable control plane | 1 个或多个 | 支持 | 外部状态机 + GitHub | 很高 | 当前过重 |
| H. 迁入组织 + GitHub Merge Queue | 0 或 1 | 支持 | GitHub queue | 中 | 高并发阶段的优先升级路线 |

### 5.1 A：零自建 App

“零 App”实际是零自建 App，因为 `GITHUB_TOKEN` 本身是 GitHub Actions App 的仓库级 installation token。[G1]

优点：

- 无私钥、无安装和轮换。
- token 按 job 生成，仅限当前仓库，可用 `permissions` 收敛。
- 适合只读分析、评论、同一 workflow 内的候选生成和人工批准流程。

关键缺口：

- `GITHUB_TOKEN` 创建或更新 PR 后，GitHub 会抑制由该 token 产生的大多数后续 workflow 事件；这与 fork 首次运行的 approval-required 机制不同。[G1]
- 可用 `workflow_dispatch`/`repository_dispatch`、同一 workflow 内跑 CI 或自定义 check 绕过该限制，但这会引入自定义编排和 required-check 绑定验证。

结论：如果目标是完全无人值守 PR CI，零 App 不一定是最简单方案。应先做短 PoC，而不是把复杂的 dispatch 链当作既定设计。

### 5.2 B：一个 Writer App

> 实施修订（2026-08-27）：下列原始最小权限结论不再适用于当前实现。由于普通协作者可更新未受保护的 `agent/**` 分支，Publisher 需要同一 Writer App 的 `checks:write` 创建精确 head publication attestation；Finalizer 以 App 身份和 canonical payload 验证该证明。`docs/single-writer-autonomy-implementation.md` 是现行权限与上线规范。

职责：

- 仅安装到目标仓库。
- 最小权限：`metadata:read`、`contents:write`、`pull_requests:write`。
- token 仅在 Publisher job 中生成，并进一步缩小仓库和权限。
- 可创建 Agent 和 Sync 分支/PR，并请求 native auto-merge。

禁止：

- 无 `administration`、`workflows:write`、`checks:write`、`statuses:write`、`secrets`、environment 或 release 权限。
- 不在 ruleset bypass 列表。
- 私钥不进入 Agent、PR CI 或 fork PR 上下文。

为什么是推荐方案：

- 解决无人值守 PR workflow 的身份问题。
- 不接管 Policy 和 merge authority。
- Agent 与 Sync 在当前单仓库内需要相同写权限，复用不会额外扩大 GitHub API 权限集合。
- 只维护一个私钥、一套安装和一条撤销路径。

### 5.3 C：Writer + Sync 两 App

优点：独立撤销、审计、密钥轮换和故障域。

缺点：

- 两者在同一仓库通常仍是相同 `contents/pull_requests:write`。
- 无法通过 App 权限把一个身份限制到 `agent/**`，另一个限制到 `automation/**`。
- 增加 bootstrap、安装回执、secret、轮换、失效恢复和配置漂移。

结论：它是风险隔离选项，不是默认架构。

### 5.4 D/G：外部控制平面

典型组成：GitHub App webhook、持久队列、状态数据库、凭据 broker/HSM、隔离 worker、审计和配额服务。

真正需要它的条件：

- 多仓库或多组织集中治理。
- 工作流要跨数小时/数天暂停、恢复和人工回调。
- 需要严格的跨仓库配额、计费、优先级或全局并发控制。
- 需要把 App 私钥留在外部 broker，通过 OIDC claim 发放单仓库、单用途短期 token。
- Actions 的事件、时限和存储模型已反复造成不可接受的重复或恢复失败。

当前没有这些证据。现在引入外部控制平面会把 GitHub 原生问题变成服务可用性、数据库迁移、备份、密钥托管和 on-call 问题。

### 5.5 E：托管 AI coding agent

托管 Agent 适合作为可替换的“候选生成器”，不应改变合并治理。

| Agent | 官方确认的关键边界 | 对无人值守合并的影响 | 适用判断 |
|---|---|---|---|
| GitHub Copilot cloud agent | 每任务 ephemeral Actions 环境、单分支写入、默认防火墙；Draft PR 必须人工 review/merge，Agent 不能 approve/merge | 产品硬边界不满足完全无人值守 merge | 若接受人工 code review，它是最省运维的候选 |
| OpenAI Codex Cloud | 隔离容器；Agent 阶段默认断网；secret 只给 setup，Agent 前移除；结果以 diff/PR 交付 | 直连模式的 PR 身份、token TTL 和禁止 merge 合约公开资料不足 | 适合 PoC，安全基线较强 |
| Claude Code GitHub Actions | 运行在 Actions；可自定义权限、触发器和工具；支持自建最小 App | 安全边界由 workflow 作者负责，不是不可绕过的产品约束 | 灵活，适合需要无人值守 PR 的自有工作流 |
| Cursor Cloud Agents | Firecracker microVM、OIDC、secret 分层、网络 allowlist；普通任务创建 Draft PR | 自动化权限需主动关闭 approve/review 能力 | 托管候选，可作为 Codex/Claude 对照 PoC |
| Devin | 干净 VM、日志和 OIDC；GitHub App 与功能面较广 | 主要依靠 branch protection，默认权限面偏大 | 对单 fork 过重 |
| OpenHands | Cloud 或自托管，支持 Docker sandbox；Cloud GitHub App 包含 Actions/Workflows RW | 自托管能控制但需承担隔离、补丁和监控；Cloud 权限面较大 | 仅在数据/模型/运行环境必须自控时采用 |
| Jules | 短生命周期 VM、定时和 issue 触发；网络与 secret 边界公开信息较弱 | 适合公开、无 secrets 的任务，不宜承担敏感自动合并 | 备选，不进入首轮 shortlist |

首轮 PoC 建议只比较两种执行模型：

1. **托管隔离型**：Codex Cloud 或同等产品，验证网络、secret、PR 身份和审计。
2. **仓库工作流型**：Claude Code Action 或同等 CLI，验证最小权限、publisher 分离和成本上限。

GitHub Copilot cloud agent 单独作为“强制人工 review”基线，不把其约束误写成可无人值守 merge。

### 5.6 F：SaaS merge/controller

Mergify 等 SaaS 可以提供规则、优先级、批处理和队列，但需要第三方高权限 GitHub App，并增加供应商控制面。[M1]

当前单 fork、低并发场景中：

- native auto-merge 已覆盖“条件满足后合并”。
- strict required checks 已覆盖低并发 base 漂移。
- 没有批量、优先级或 speculative queue 的业务证据。

因此不引入 SaaS Merger。只有在无法使用 GitHub Merge Queue、又确有复杂队列需求时才重新比较 Mergify 等产品。

### 5.7 H：组织仓库 + GitHub Merge Queue

这是未来高并发的优先路线，而不是当前前置条件。

升级触发器：

- 同一目标分支经常存在多个已绿 PR。
- strict checks 因 base 频繁更新而大量重复运行。
- 两个单独通过的 PR 合入后互相破坏成为常见故障。
- 迁入组织和套餐成本可接受。

采用时 required workflow 必须监听 `merge_group`，并在 GitHub 创建的候选 merge group 上报告 checks。[G6]

## 6. 推荐架构详细设计

### 6.1 信任域

| 域 | 信任级别 | 可访问内容 | 禁止内容 |
|---|---|---|---|
| Trigger/Planner | 低到中 | issue、任务元数据、只读源码 | GitHub 写 token、release secret |
| Agent sandbox | 不可信计算 | checkout、测试工具、必要依赖 | App private key、生产凭据、merge 权限 |
| Publisher | 高 | 候选 patch、目标 head/base、短期 App token | 模型自由执行、任意网络、release secret |
| PR CI | 不可信代码执行 | PR candidate、只读 token | repository/environment secrets、写 token |
| Policy check | 高且确定性 | GitHub API 只读状态、diff 元数据、CI 结论 | 模型输出作为授权、任意脚本输入拼接 |
| GitHub merge authority | 最终权威 | ruleset、checks、reviews、conflict、head/base | bypass Writer |
| Release | 高且人工 | release environment secrets | Agent 自动批准 |

### 6.2 Publisher 需要做的确定性校验

Publisher 不信任 Agent 产物，应至少验证：

- task id、base SHA、输入来源和允许的仓库一致。
- patch 可干净应用，提交树与生成后的预期 tree SHA 一致。
- 文件数量、总 diff、二进制大小和单任务时限在硬上限内。
- 不修改 `.github/workflows/**`、权限/认证、release、部署、secret、CODEOWNERS、ruleset 管理文件等敏感路径，除非任务进入人工 lane。
- 分支名属于固定命名空间，且目标 ref 写入使用 exact old-SHA lease 或写前重读。
- 只创建/更新该 task 管理的 PR，不接管人工 PR。
- 每次更新记录 task、session、candidate commit、base SHA 和 run URL。

### 6.3 Policy required check

Policy 是一个稳定名称、确定性的 required job。它只基于可复算事实：

- PR 来源是受管分支和受信 Writer 身份。
- head SHA 与接受审查/CI 的 SHA 一致。
- base 满足 strict up-to-date 要求，或未来由 merge queue 生成候选 SHA。
- Policy 只计算当前 diff 的确定性资格 `eligible/manual/deny`，不汇总或代理其他 CI；Rust、Frontend、Policy 等 required checks 由 GitHub 保护规则直接聚合。
- 所有 required CI 来自 expected source 且状态为允许值；Policy workflow 必须来自受保护 base，不能执行 PR head 中的策略代码。
- diff 只包含低风险 allowlist 路径和变更类型。
- 无 merge conflict、无未解决讨论、无阻塞 review。
- PR 不是 draft，或者由受信 Publisher 在条件满足后转为 ready。
- 没有 `manual-only`、`security-review`、`release`、`conflict` 等阻塞标记。
- 没有超预算、凭据异常、来源异常或状态读取不完整。

模型风险评分只能用于增加阻塞，不能单独放行。

### 6.4 Auto-merge

推荐流程：

1. Publisher 创建或更新 PR。
2. PR CI 和 Policy check 对准确 head SHA 运行。
3. 自动化请求启用 GitHub auto-merge，merge method 固定为 squash。
4. GitHub 持续检查保护规则；条件漂移时不合并。
5. 实际合并后，审计记录关联 PR、head、base、checks 和最终 merge commit。

不要实现“检查完再直接调用 merge API”的自建 Merger。那种读后写流程在 head push、base 更新、审批撤回和 check 更新之间存在竞态。

## 7. 上游同步方案

### 7.1 推荐模式

采用 scheduled + `workflow_dispatch` 的固定同步 PR：

- 固定 head：例如 `automation/sync-upstream-main`。
- 固定 base：fork `main`。
- 每次记录 `upstreamSha`、`forkBaseSha`、candidate SHA 和 run URL。
- 查询并复用同一开放 PR，不按天创建新 PR。
- 仅 force-update 自动化自有 head，禁止 force-push `main`。
- 无差异时 no-op；不修改普通人工 PR。

### 7.2 为什么不直接调用 Sync Fork 写 main

GitHub Sync Fork UI/API 和 `gh repo sync` 适合人工维护，但直接更新默认分支会绕过 PR 审查、required CI 和可见冲突处理。[S1][S2]

定时 PR 模式提供：

- GitHub 原生三方合并和冲突可见性。
- 统一 CI/Policy/auto-merge。
- 完整 timeline 和回滚依据。
- 可复用分支/PR，避免每日 PR 膨胀。

### 7.3 冲突与历史重写

- 普通无冲突同步：进入低风险自动合并 lane。
- Git 冲突：保留 PR、标记 `sync-conflict`、停止自动合并；禁止自动选择 ours/theirs。
- 上游非快进或 force-push：视为历史重写，停止并要求语义复核。
- base/head 在验证期间变化：旧结论作废，重新生成候选。
- 人工关闭同步 PR：记录 tombstone；无论上游 SHA 是否变化，只有维护者显式 dispatch `resume` 才可重开。
- 持续 CI 失败：有界重试基础设施错误；代码失败进入待处理状态，不无限调用 AI。

### 7.4 不采用的同步模式

| 模式 | 不采用原因 |
|---|---|
| 自动 rebase + force-push 默认分支 | 重写公开历史，破坏开放 PR 和下游 clone |
| `git push --mirror` | 适合只读镜像，会覆盖本地 refs，不保留 fork 补丁 |
| subtree/submodule | 适合组件依赖，不适合整仓 fork 追踪 |
| patch queue | 适合长期少量显式补丁，但增加补丁重放维护；可作为未来 fork 偏离很大时的迁移选项 |
| Renovate/Dependabot | 是依赖更新器，不是通用 upstream fork 同步器 |

## 8. 自动合并的风险分层

### 8.1 可进入无人值守 lane

必须同时满足确定性 allowlist，例如：

- 文档、测试、lint、格式化或已定义的机械性更新。
- 依赖更新仅限允许范围，且 lockfile、许可证和安全扫描满足策略。
- 上游同步无冲突、无敏感路径、required CI 全绿。
- 变更量、文件类型、目录和生成器来源在硬限制内。
- 无 release、部署、认证、权限、加密、数据迁移或供应链根配置变化。

### 8.2 必须转人工 lane

- `.github/workflows/**`、Actions 权限、App、ruleset、CODEOWNERS、release 配置。
- 认证、授权、secret、密码学、计费、生产数据、数据库破坏性迁移。
- 新的外部网络目的地、二进制制品或不可验证生成代码。
- 模型自己声称“低风险”但确定性分类器无法证明。
- 任何 Git 冲突、上游历史重写、检查来源异常或状态读取不完整。

## 9. 威胁模型与最低安全基线

| 威胁 | 主要路径 | 最低控制 | 残余风险 |
|---|---|---|---|
| Prompt injection | issue、PR、评论、README、网页、依赖文档 | 只接受可信 actor/标签；工具 allowlist；模型无写 token；默认无网 | 允许的内容仍可诱导错误 patch |
| Secret exfiltration | Agent shell、依赖脚本、网络、日志 | Agent 仅接触模型访问凭据或本地 API proxy，不接触 GitHub Writer/release secret；`drop-sudo`/非特权用户；egress allowlist | 模型凭据和允许域仍是风险面，必须单独轮换、限额和审计 |
| 恶意 PR code | `pull_request_target`、`workflow_run`、共享 cache/artifact | PR CI 用 `pull_request`、只读 token、无 secrets；特权 job 不 checkout 不可信 head | 测试依赖本身仍可攻击 runner |
| 第三方 Action 被劫持 | 可移动 tag、恶意更新 | 完整 commit SHA pin；Dependabot/审计 | 固定 SHA 本身可能已经恶意 |
| required check 伪造 | 同名 status/check | expected source 绑定可信 App | CI 发布者被攻陷仍可放行 |
| Writer key 泄露 | repo secret、日志、Agent 环境 | 私钥只在 Publisher environment；短期 installation token；轮换与撤销 | App 仍具仓库级 contents write |
| TOCTOU merge | head/base/check/review 在决策后变化 | native auto-merge；strict checks；写前重读 | GitHub 平台故障属于外部风险 |
| self-hosted runner 持久化 | 公共 PR 执行恶意代码 | 公共 PR 默认 GitHub-hosted ephemeral runner | 依赖 GitHub 托管隔离声明 |
| cache/artifact 投毒 | 非特权 job 影响特权 job | 特权 job验证 digest、schema、来源和 head；不执行 artifact | 验证器漏洞 |
| 无限 Agent 循环/费用失控 | CI 失败反复修复 | 每任务 turn/time/retry/concurrency 硬上限 | 即使月度预算不设上限，也需要运行级熔断 |

GitHub 官方明确警告：`pull_request_target` 和特权 `workflow_run` 若 checkout 或执行不可信 PR 内容，可导致仓库接管；第三方 Actions 只有完整 commit SHA 是不可变固定方式。[G8]

Codex 官方明确说明：Agent 阶段默认断网，启用网络会带来 prompt injection、代码/secret 外泄、恶意依赖和许可证风险；应限制域名与 HTTP 方法。[A2]

GitHub Copilot 官方也说明其 firewall 不覆盖 setup steps、MCP 和环境外进程，并存在绕过可能。因此防火墙是减缓措施，不是完整安全边界。[A1]

## 10. 合并控制器与跨平台参考

### 10.1 合并控制器

| 方案 | 当前状态 | 结论 |
|---|---|---|
| GitHub native auto-merge | 平台原生 | 当前推荐 |
| GitHub Merge Queue | 平台原生，面向高并发 | 迁入组织后的优先升级路线 |
| Mergify | 活跃 SaaS | 原生能力不足且确需高级队列时再评估 |
| bors-ng | 官方仓库已归档，并建议迁移 GitHub Merge Queue | 不采用 |
| Homu | 官方仓库已归档 | 仅作历史设计参考 |
| Prow/Tide | 活跃、面向 Kubernetes 规模 | 当前过重 |
| Zuul | 成熟的投机 gating 系统 | 当前过重 |
| Kodiak | 当前维护和服务证据不足 | 不进入 shortlist |

### 10.2 跨平台可迁移模式

| 平台 | 机制 | 可迁移启示 |
|---|---|---|
| GitLab | merged-results pipeline + merge trains | 校验候选合并结果；前序项变化使后续候选失效 |
| Azure DevOps | branch policies + build validation + auto-complete | 合并策略是服务端权威；验证应有 base 变化失效规则 |
| Gerrit | submit requirements + submit strategy | “能否提交”与“如何写入目标分支”分离 |

这些参考支持 GitHub 原生治理路线，不构成迁移平台的理由。[X1][X2][X3]

## 11. 分阶段采用路线

### 阶段 0：保持暂停并做最小 PoC

只验证，不改变生产治理：

1. 证明 `GITHUB_TOKEN` 创建 PR 后当前仓库的后续 workflow 事件抑制行为，并与 fork approval-required 明确区分。
2. 证明一个最小 Writer App 创建 PR 后 required CI 可无人批准触发。
3. 证明 required check expected source 可绑定 GitHub Actions，且同名伪造不通过。
4. 证明 native auto-merge 在 head/base/check/review 漂移时保持 fail closed。
5. 证明一个固定同步分支和 PR 在连续三次 schedule 中可幂等复用。

### 阶段 1：原生门禁，不接入 AI 写入

- 固化 ruleset/branch protection、required CI、Policy check 和 release environment。
- 建立敏感路径 manual lane。
- 只运行 read-only Agent 或本地候选生成。

### 阶段 2：一个 Writer App

- 仅安装到目标仓库。
- 只开放受控 Publisher job。
- 先允许文档/测试类 PR，不自动合并生产逻辑。
- 观察审计、失败恢复和 App 撤销演练。

### 阶段 3：低风险 auto-merge

- 仅对已定义 allowlist 启用。
- 持续收集 false-positive/false-negative、回滚和 CI 重跑数据。
- release 继续人工审批。

### 阶段 4：按证据升级

- Agent/Sync 需要硬隔离时才拆第二 App。
- 高并发时迁入组织并启用 Merge Queue。
- 多仓库、持久任务、集中配额或凭据 broker 成为真实需求时才建设外部控制平面。

## 12. 验证计划与接受标准

| 验证项 | 实验 | 接受标准 |
|---|---|---|
| 身份最小化 | 比较 `GITHUB_TOKEN` 与 Writer App 创建的 PR | Writer App 路径无需人工批准即可运行 required CI；无额外权限 |
| Token 隔离 | 在 Agent job 探测 secret/token 可见性 | Agent 不能读取 App private key 或 installation token |
| 分支约束 | 尝试写受管分支、其他分支、`main` | 受管分支成功、`main` 由保护规则拒绝；如其他未保护分支也可写，必须记录为 Writer App 的平台残余能力并由 Publisher 校验或 ruleset 缓解，不能声称硬隔离 |
| Check 来源 | 用不同身份提交同名状态 | 非 expected source 不满足 required check |
| Head/base 漂移 | CI 后更新 PR 或 base | 旧 Policy 失效，GitHub 不合并 |
| 冲突 | 构造文本和语义冲突 | 自动化停止、保留证据、不选边 |
| 幂等 | 重放 webhook、rerun、并发 schedule | 只有一个管理分支和一个开放 PR |
| 凭据撤销 | 撤销 App 安装/轮换私钥 | 自动化 fail closed，恢复后不重复写入 |
| Agent 网络 | 注入外泄测试域与非允许 HTTP 方法 | 请求被阻断并有审计记录 |
| Release 隔离 | Agent PR 尝试触发 release | release job 仍需人工 environment approval |
| 故障恢复 | GitHub 5xx、runner 中断、超时 | 有界重试；超过阈值进入稳定阻塞状态 |

Desk research 不能替代以上实验。在 PoC 全部通过前，不应声称“无人值守安全合并已实现”。

## 13. 覆盖面审计与自动扩展

### 13.1 审计结论

本报告的初始限定“单一公开、个人账户 GitHub fork”对**当前落地决策**是合适的，不应为了追求大全而直接扩大为企业平台建设。但如果把报告理解为“业界自主软件交付架构全景”，初始覆盖确实偏窄。

自动审计使用了以下维度：触发与任务入口、Agent 执行、隔离、身份、网络、secret、策略、CI、合并一致性、上游同步、持久编排、供应链、审计、数据治理、成本、事故恢复、跨仓库和 release。结果如下：

| 维度 | 初始覆盖 | 是否扩展 | 扩展后的判断 |
|---|---|---|---|
| GitHub 原生身份/保护/合并 | 充分 | 否 | 已能支撑当前推荐 |
| AI Agent 产品与执行模型 | 充分 | 否 | 已覆盖托管、Actions 型和自托管代表 |
| Fork upstream sync | 充分 | 否 | 已覆盖 PR、merge/rebase、mirror、patch queue 等 |
| Merge controller / queue | 充分 | 否 | 已覆盖 native、SaaS 和大型自托管代表 |
| 跨平台门禁 | 充分 | 否 | GitLab/Azure/Gerrit 足以提炼共同模式 |
| Policy-as-code | 偏薄 | **是** | 补充 OPA/Conftest，但当前无需引入独立 policy engine |
| 短期 GitHub 凭据 broker | 偏薄 | **是** | 补充 OIDC-to-GitHub STS；仅在外部 worker/多仓库时采用 |
| Durable orchestration | 偏薄 | **是** | 补充 Temporal 类 event-history 模式和采用触发器 |
| 自托管 runner 隔离 | 中等 | **是** | 补充 ARC/单次 runner；当前公开 PR 仍优先 GitHub-hosted |
| 供应链 provenance | 中等 | **是** | 补充 SLSA/attestation 的边界，主要适用于 release artifact |
| 审计、数据保留与隐私 | 偏薄 | **是** | 补充个人账户 security log 与组织 audit log 差异 |
| 灾备、撤销与熔断 | 中等 | **是** | 补充 kill switch、RTO/RPO 和故障演练 |
| 非 GitHub SCM/多仓库平台 | 有限 | 条件扩展 | 当前不影响决策，作为未来触发式范围，不展开产品清单 |
| 合规、数据驻留、法律 | 有限 | 条件扩展 | 当前没有地域/法规要求，不作合规结论 |

### 13.2 Policy-as-code

Policy 可以有三种复杂度：

1. **普通脚本/程序 + required job**：当前推荐。规则少、数据源固定、团队小，容易测试和审阅。
2. **OPA/Conftest**：规则跨多个 workflow、仓库或结构化配置复用时采用。OPA 官方支持在 CI/CD 中对结构化数据实施 policy-as-code 和测试。[P1]
3. **外部 policy decision point**：只有多个调用方必须共享实时策略、需要中央发布/回滚和决策日志时采用。

不应仅因“业界有 OPA”就引入 Rego。对单仓库，独立 policy engine 会增加语言、bundle、版本和调试成本，且 GitHub required check 仍然是最终强制点。

### 13.3 OIDC 到 GitHub 短期凭据 broker

外部 Agent/worker 如果不能安全持有 GitHub App private key，可以采用 STS 模式：worker 提交 OIDC token，broker 校验 issuer/subject/claims 和仓库中的 trust policy，再签发被缩小权限的 GitHub installation token。`octo-sts/app` 是这一模式的开源代表。[I1]

适用条件：

- worker 位于 GitHub Actions 之外，且能提供可信 OIDC identity。
- 多仓库需要集中撤销和 policy-as-code token issuance。
- App 私钥需要留在 HSM/broker，不能作为 repository secret。

当前一个仓库的 Publisher job 直接生成短期 installation token 更简单。没有必要为了消除一个受保护的 App private key而部署持续在线 STS 服务。

### 13.4 Durable orchestration

Actions 已足以处理分钟到小时级、可重跑的 PR 自动化。Temporal 一类 durable execution 平台通过 event history 重建状态、在进程或基础设施失败后继续执行，适合长生命周期、多阶段等待和补偿流程。[D1]

采用 durable control plane 的客观触发器：

- 任务要等待数天的人机回调或外部系统。
- 单任务跨多个仓库/服务，必须有补偿和 exactly-once effect 近似。
- Actions artifact/run retention 不足以恢复状态。
- Webhook 重放和 runner 中断已经造成重复外部副作用。
- 有专门团队承担数据库、worker、升级、备份和 on-call。

在触发器出现前，固定 PR、GitHub timeline、run logs 和显式状态标签已经构成更简单的持久记录。

### 13.5 Runner 与网络隔离

执行策略按风险排序：

1. 公开 PR CI：GitHub-hosted ephemeral runner，read-only token，无 secrets。
2. 可信默认分支 Publisher：GitHub-hosted runner + protected environment + 最小短期 token。
3. 必须访问内网时：单次使用、不可复用的 self-hosted runner；GitHub 对 Copilot self-hosted 环境也推荐 ephemeral single-use runner，常见实现是 ARC 或 Runner Scale Set Client。[E1]
4. 不使用长期在线、跨仓库共享的普通 self-hosted runner执行不可信代码。

如果需要更细网络证据，可评估 runner egress 监控/阻断产品；它们是 defense in depth，不替代无 secret 和短期 token。

### 13.6 供应链与制品

SLSA 将 source、build 和 provenance 威胁分层；GitHub artifact attestation 可以证明制品由哪个 workflow、哪个源码上下文生成。[L1][G10]

边界必须明确：

- PR 是否可合并仍由 CI、Policy 和 ruleset 决定。
- Attestation 证明来源，不证明生成逻辑没有漏洞，也不证明 Agent 的修改语义正确。
- 当前日常 PR 不需要把 attestation 设为前置依赖。
- 发布二进制、容器或 release assets 时，应在人工 release lane 生成并消费侧验证 provenance。

### 13.7 审计、数据治理和事故恢复

最低审计事件：trigger actor/source、task id、Agent vendor/model、session/run、base/head/tree SHA、Publisher identity、token scope、Policy version/result、required checks、auto-merge request、最终 merge/revert、成本与失败原因。

个人账户的 GitHub security log 和组织 audit log 能力不同。若未来需要集中导出、长期保留、SIEM、团队权限和组织级审计，迁移组织本身会成为架构触发器，而不是单纯为了 Merge Queue。[O1][O2]

最低事故控制：

- 一个仓库级 kill switch 可阻止新的 Agent task、Publisher 写入和 auto-merge 请求，但不能撤销已签发 token，也不会自动取消已 armed 的 auto-merge；紧急停机还必须显式 disarm/close managed PR 并 suspend/uninstall App。
- App installation 可立即 suspend/uninstall，private key 有轮换 runbook。
- required Policy check 默认 fail closed；不得通过删除 required check 来“临时恢复”。
- 自动合并事故以受保护 revert PR 恢复，不直接 force-push `main`。
- 每季度演练：App 撤销、GitHub 5xx、stale head、伪造 check、Agent 外泄、同步冲突和 release 越权。
- 单仓库阶段目标可设为：停止新写入 RTO 15 分钟；GitHub 中的 PR/commit/audit 元数据 RPO 0；外部日志 RPO 取决于是否接入日志存储。该数值是建议，实施时需由 owner 接受。

### 13.8 明确不扩展的范围

以下范围现在扩大不会改变决策，反而会稀释报告：

- Kubernetes 多租户平台、service mesh、跨区域 control plane。
- GitHub Enterprise Server、GitLab/Bitbucket 实施迁移。
- SOC 2、ISO 27001、GDPR、数据驻留或行业法规合规结论。
- 模型 benchmark、代码质量排名和供应商市场份额。
- 生产部署、生产数据和 release secret 自动化。

当出现多仓库、组织治理、私有代码驻留、监管要求或持续高并发中的任一事实时，应重新开启对应专题调研，而不是把它们提前实现。

## 14. 最终决策建议

### 14.1 现在应决定的内容

1. 接受“一个 Writer App 是无人值守 PR CI 的最小身份”，而不是继续追求形式上的零 App。
2. 同一个 Writer App 默认服务 Agent 和 Sync；只有明确安全边界证据才拆分。
3. Policy 和 merge authority 永久留在 GitHub 原生机制。
4. AI executor 保持可替换，先做两个小型 PoC，不把某个供应商嵌入治理核心。
5. 自动合并只对确定性低风险 allowlist 开放，冲突和语义不确定转人工。

### 14.2 现在不应做的内容

- 不创建四个 App。
- 不自建 Merger、merge queue、receipt authority 或 bootstrap 控制平面。
- 不把 App 私钥交给 Agent。
- 不用 PAT 作为长期自动化根身份。
- 不因“预算暂时无上限”取消每任务时限、turn、并发和重试硬上限。
- 不把 release 人工审批并入无人值守 Agent 环境。

## 15. 不确定项与后续核验

- 不同托管 coding agent 的 PR author、token TTL、网络隔离、数据保留和 merge 禁止边界仍可能变化，实施前需逐产品复核。
- Codex Cloud 直连 GitHub 的写入身份和服务端 merge 权限公开资料不足，标记为待 PoC。
- Cursor、Devin、Jules 的部分安全与价格信息来自供应商文档，未做独立审计。
- Mergify 的高级队列能力已从官方文档确认，但没有证据证明当前单 fork 需要它。
- GitHub 计划、个人/组织所有权和功能可用范围会变化，迁移组织或启用新规则前需实时验证。
- GitHub repository App 权限不提供可靠的分支前缀 capability；推荐架构依赖保护规则、无 bypass、Publisher 校验和审计共同约束。

## 16. 来源索引

访问日期均为 `2026-08-20`。除 `[R1]` 外均为公开官方资料。

### 16.1 当前 GitHub 事实

- `[R1][A]` GitHub repository API / `gh repo view`：`https://github.com/JinPengGeng/aeris-token`

### 16.2 GitHub 原生治理与安全

- `[G1][B]` `GITHUB_TOKEN` 语义与事件行为：<https://docs.github.com/en/actions/concepts/security/github_token>
- `[G2][B]` Workflow 权限与 concurrency：<https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax>
- `[G3][B]` GitHub App installation token：<https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-an-installation-access-token-for-a-github-app>
- `[G4][B]` Ruleset 可用规则与 expected source：<https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/available-rules-for-rulesets>
- `[G5][B]` Native auto-merge：<https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/incorporating-changes-from-a-pull-request/automatically-merging-a-pull-request>
- `[G6][B]` GitHub Merge Queue：<https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/managing-a-merge-queue>
- `[G7][B]` GitHub Environments：<https://docs.github.com/en/actions/deployment/targeting-different-environments/using-environments-for-deployment>
- `[G8][B]` GitHub Actions secure use：<https://docs.github.com/en/actions/reference/security/secure-use>
- `[G9][B]` OpenID Connect：<https://docs.github.com/en/actions/concepts/security/openid-connect>
- `[G10][B]` Artifact attestations：<https://docs.github.com/en/actions/concepts/security/artifact-attestations>

### 16.3 AI coding agents

- `[A1][B]` GitHub Copilot cloud agent：<https://docs.github.com/en/copilot/concepts/agents/cloud-agent/about-cloud-agent>
- `[A1a][B]` Copilot 风险与缓解：<https://docs.github.com/en/copilot/concepts/agents/cloud-agent/risks-and-mitigations>
- `[A1b][B]` Copilot firewall：<https://docs.github.com/en/copilot/how-tos/copilot-on-github/customize-copilot/customize-the-firewall>
- `[A2][B]` OpenAI Codex Cloud：<https://learn.chatgpt.com/docs/cloud>
- `[A2a][B]` Codex Cloud environment：<https://learn.chatgpt.com/docs/environments/cloud-environment>
- `[A2b][B]` Codex Agent internet access：<https://learn.chatgpt.com/docs/cloud/internet-access>
- `[A2c][B]` Codex GitHub review integration：<https://learn.chatgpt.com/docs/third-party/github>
- `[A3][B]` Claude Code GitHub Actions：<https://code.claude.com/docs/en/github-actions>
- `[A3a][B]` Claude Code Action security：<https://github.com/anthropics/claude-code-action/blob/main/docs/security.md>
- `[A4][C]` Cursor Cloud Agent security：<https://cursor.com/docs/cloud-agent/security>
- `[A5][B/C]` OpenHands GitHub Integration：<https://docs.openhands.dev/openhands/usage/cloud/github-installation>
- `[A5a][B]` OpenHands sandbox overview：<https://docs.openhands.dev/openhands/usage/sandboxes/overview>
- `[A6][C]` Devin GitHub integration：<https://docs.devin.ai/integrations/gh>
- `[A7][C]` Google Jules FAQ：<https://jules.google/docs/faq>

### 16.4 上游同步与 Git

- `[S1][B]` GitHub Syncing a fork：<https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/working-with-forks/syncing-a-fork>
- `[S2][B]` REST Sync a fork branch：<https://docs.github.com/en/rest/repos/forks?apiVersion=2022-11-28#sync-a-fork-branch-with-the-upstream-repository>
- `[S3][B]` `gh repo sync`：<https://cli.github.com/manual/gh_repo_sync>
- `[S4][B]` Git merge：<https://git-scm.com/docs/git-merge>
- `[S5][B]` Git rebase：<https://git-scm.com/docs/git-rebase>
- `[S6][B]` Git subtree：<https://git-scm.com/docs/git-subtree>
- `[S7][B]` Git submodule：<https://git-scm.com/docs/git-submodule>
- `[S8][B]` Renovate automerge 定位：<https://docs.renovatebot.com/key-concepts/automerge/>

### 16.5 合并控制器

- `[M1][C]` Mergify Merge Queue：<https://docs.mergify.com/merge-queue/>
- `[M2][B]` bors-ng，已归档：<https://github.com/bors-ng/bors-ng>
- `[M3][B]` Homu，已归档：<https://github.com/rust-lang/homu>
- `[M4][B]` Prow/Tide：<https://docs.prow.k8s.io/docs/components/core/tide/>
- `[M5][B]` Zuul gating：<https://zuul-ci.org/docs/zuul/latest/gating.html>

### 16.6 跨平台门禁

- `[X1][B]` GitLab merged-results pipelines / merge trains：<https://docs.gitlab.com/ci/pipelines/merged_results_pipelines/>、<https://docs.gitlab.com/ci/pipelines/merge_trains/>
- `[X2][B]` Azure DevOps branch policies / complete PR：<https://learn.microsoft.com/en-us/azure/devops/repos/git/branch-policies?view=azure-devops>、<https://learn.microsoft.com/en-us/azure/devops/repos/git/complete-pull-requests?view=azure-devops>
- `[X3][B]` Gerrit submit requirements / project configuration：<https://gerrit-review.googlesource.com/Documentation/config-submit-requirements.html>、<https://gerrit-review.googlesource.com/Documentation/project-configuration.html>

### 16.7 扩展架构模式

- `[P1][B]` OPA in CI/CD：<https://www.openpolicyagent.org/docs/cicd>
- `[I1][B]` Octo STS，OIDC 到 GitHub installation token：<https://github.com/octo-sts/app>
- `[D1][B/C]` Temporal Workflow 与 event history：<https://docs.temporal.io/workflows>
- `[E1][B]` GitHub Actions Runner Controller：<https://docs.github.com/en/actions/concepts/runners/actions-runner-controller>
- `[L1][B]` SLSA v1.2 specification：<https://slsa.dev/spec/v1.2/>
- `[O1][B]` GitHub personal security log：<https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/reviewing-your-security-log>
- `[O2][B]` GitHub organization audit log：<https://docs.github.com/en/organizations/keeping-your-organization-secure/managing-security-settings-for-your-organization/reviewing-the-audit-log-for-your-organization>

## 17. 调研限制与工具记录

- 原始候选池来自 GitHub 搜索，存在查询词命中 README 噪声、重复项目和流行度偏差，仅用于发现。
- Agent Reach 的 GitHub CLI 后端可用并用于当前仓库、官方仓库和维护状态核验。
- Agent Reach 主命令在本机 PATH 中不可用；Exa MCP server 未配置；Jina Reader 请求出现 TLS/401，因此没有把这些失败路径当作证据。
- 对普通网页改用官方 Markdown endpoint、官方 HTML、官方仓库和 GitHub API。
- 没有使用社交媒体观点作为架构结论依据，因为这些结论已有更高等级的一手来源。
- 本报告只完成研究和规划输入；后续工作树已加入单 Writer 实现，但远端设置、运行时与安全验收仍须以实施规格、测试和 GitHub 现场证据确认。
