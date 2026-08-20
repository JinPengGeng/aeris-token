# Writer activation runbook

Writer 代码和 workflow 默认关闭。未完成本页的 App provisioning 与现场 canary 前，不得修改 `.github/agents.yml`、`.github/automation-policy.yml` 中的 `writer.enabled: false`，也不得设置 `AERIS_WRITER_ENABLED=true`。

## 已完成的代码侧门禁

- `writer.yml` 固定从默认分支运行 `preflight -> analyze -> build -> publish`，按 Issue 串行且所有 `GITHUB_TOKEN` 权限只读。
- Writer App 私钥只在 `publish` job 的 `writer` Environment 中引用；disabled/terminal canary step 不引用该 Secret，也不 mint token。
- `preflight`、candidate、receipt 绑定精确 comment/Issue URL、Issue `updated_at`、标签、命令、actor、policy/config SHA、base/source SHA、Issue ref 和 lease expiry。
- build 只接受 allowlist 中的普通文本 add/modify，拒绝 rename/delete、symlink/submodule、可执行位、`.github/**` 和其他敏感路径，并按变更族执行可信测试。
- publish 只通过 App token 的 Git push 创建或推进 Writer ref，并固定使用 `--force-with-lease=refs/heads/agent/issue-N:<expected-old-sha>`；创建时 expected old 为空。lease 不匹配会由 Git 原子拒绝，不使用缺少 old-SHA 条件的 REST ref PATCH。每次 push 和 Draft PR create 之前仍再次执行现场 authorization；不会 review、approve、merge、ready、close、delete 或补偿性回滚。
- `/agent retry-write` 的 fix cycle 不接受 workflow input 或环境变量。PR body 中 canonical v2 receipt marker 必须恰好是 `fix_cycle=0` 的签名 origin anchor，绑定首次命令 comment、受信 lifecycle epoch、candidate SHA 与 commit SHA，且后续不改写；任何签名正确但 cycle 非 0 的 marker 也拒绝。preflight 用 `AERIS_WRITER_PUBLIC_KEY` 验签，并通过 GitHub compare 从 anchor commit 到 exact head 验证线性、单父提交 ledger；每个 retry commit message 必须严格绑定连续 cycle、触发 comment、candidate SHA 与 push 前 canonical detailed PR metadata SHA-256，且 write intent 同时绑定该摘要。旧 v1 marker、epoch 不匹配、merge commit、缺口、重复 comment、metadata 摘要不一致或非 Writer 格式提交均无效。publish 在 mint token 前确认公钥与 `AERIS_WRITER_PRIVATE_KEY` 匹配。首次 implement 为 cycle 0，后续不同 comment 只能递增到 1、2；同一 comment 重放和第 3 次 retry 均 fail closed。
- 每次判断 managed PR lifecycle 都必须完整读取该 PR 的 authoritative Issue Timeline。任意历史 `closed` 事件都会在受信 epoch 0 中永久 tombstone 对应 Issue，即使 PR 后来 reopened 也不得 retry、推进 ref 或更新 PR；timeline 无权限、API 失败、响应畸形或达到三页上限而截断时一律 fail closed。永久 tombstone 不通过 receipt 自报，也不会因 branch 删除、重建或旧 receipt 仍可验签而重置。
- mutation callback 完成命令、Issue、权限等现场读取后，还会最后读取并精确验证 main、issue ref 和完整 Draft PR ownership/lifecycle/metadata 快照；PR 列表只枚举 number，快照必须来自 canonical `GET /pulls/{number}` detailed response，不能依赖会省略 `maintainer_can_modify` 的列表对象。快照覆盖 title/body/state/draft/lock、labels、milestone、assignee/assignees、requested users/teams、`maintainer_can_modify`、`auto_merge`、merge queue、base/head 与 App identity，create 后立即复读 main/ref/PR 与 receipt marker。现有 Draft PR metadata 没有 GitHub 原生 CAS，因此 retry 采用 branch-only contract：不发送 metadata PATCH，push 前要求上述快照逐字段一致，push 后除 GitHub 派生的 head SHA 外不得变化，任何人工或并发 metadata 漂移均 fail closed。
- retry 只用 `--force-with-lease` 将 exact managed ref 从已验证 old SHA 推进到内容绑定 commit；成功 receipt 为 `draft_updated` / `branch_updated_metadata_preserved`，其内嵌 candidate、candidate SHA 与 commit SHA 是本轮不可变 artifact 证据，不宣称 PR body receipt 已更新。push 后崩溃的重入必须同时验证签名 origin anchor、完整 commit ledger、当前 detailed PR metadata 摘要与提交中保存的 push 前摘要，成功时返回同一逻辑 receipt且不再次 push；无法证明时返回 `residue` 或 fail closed。

## Provisioning

1. 创建私有 GitHub App，权限严格设为 `Metadata: read`、`Contents: write`、`Pull requests: write`，关闭 webhook，并确认 App 无 `main` branch-protection bypass。
2. 仅将 App 安装到本仓库，repository selection 必须为 `selected` 且 installation 中只有本仓库。
3. 创建 `writer` Environment，仅允许默认分支部署、禁止管理员绕过，不配置宽泛 environment reviewer 例外。
4. 在 `writer` Environment 中设置 Secret `AERIS_WRITER_PRIVATE_KEY`；在 repository Variables 设置 `AERIS_WRITER_APP_ID`、`AERIS_WRITER_APP_SLUG` 和与该私钥对应的 SPKI PEM `AERIS_WRITER_PUBLIC_KEY`，供只读 preflight 验签。私钥不得放在 repository/org Secret。
5. 为 `agent/**` 配置只允许 Writer App 更新、禁止 force-push/delete 的规则集；维护者不得直接推送或重置 Writer 分支。该规则集只保护 Writer refs，不授予 App 绕过 `main` protection 的能力。
6. 保持 `AERIS_WRITER_ENABLED` 未设置或为 false，先从默认分支手工运行 `Writer Draft PR`，输入 `canary=true`。下载 `writer-receipt-*`，确认 `state=canary`、`reason=disabled_canary`、`mutations=0`，且 job 日志没有 token mint/API write。

## 真实 App canary

真实 canary 必须使用独立的 owner-authored Issue、`agent-ready` 标签和只改 `docs/**.md` 的最小任务。独立 activation validator 测试必须证明 registry/policy 同时设置 `writer.enabled: true` 可以通过，而只启用任一侧会失败。通过普通受审 PR 同时启用两份契约；合并后再短时将 `AERIS_WRITER_ENABLED=true`，全局开关也必须已开启。然后由具有 write 以上权限的非 Bot actor 发布精确评论 `/agent implement`。

验收时必须确认：

- 仅创建 `agent/issue-<number>`，没有 force push、其他 ref、review、approval、Check、merge、ready、close 或 delete 事件。
- PR 为 open Draft，base 为当前 `main`，head 与 receipt commit SHA 一致，作者为配置的 Writer App，body 含唯一 canonical ownership marker。
- changed files 仅为 canary allowlist 路径，build 测试通过，receipt 为 `draft_created`；workflow 重放不产生第二个 PR 或第二次 ref mutation。
- 使用新 comment 执行一次 `/agent retry-write`，确认 ref 精确推进一次、PR title/body 保持不变、receipt 为 `draft_updated` 且 reason 为 `branch_updated_metadata_preserved`；随后重放同一 comment 不产生第二次 ref mutation。
- Actions audit/log 中 App token 只存在于 publish 进程，App installation 仍只包含本仓库。

canary 完成后立即将 `AERIS_WRITER_ENABLED=false`。保留 registry/policy enable 的决定必须经过单独安全评审；若任一验收项失败，保持变量关闭并人工核验 residue，禁止自动 close PR 或删除分支。
