# Writer activation runbook

Writer 代码和 workflow 默认关闭。未完成本页的 App provisioning 与现场 canary 前，不得修改 `.github/agents.yml`、`.github/automation-policy.yml` 中的 `writer.enabled: false`，也不得设置 `AERIS_WRITER_ENABLED=true`。

## 已完成的代码侧门禁

- `writer.yml` 固定从默认分支运行 `preflight -> analyze -> build -> publish`，按 Issue 串行且所有 `GITHUB_TOKEN` 权限只读。
- Writer App 私钥只在 `publish` job 的 `writer` Environment 中引用；disabled/terminal canary step 不引用该 Secret，也不 mint token。
- `preflight`、candidate、receipt 绑定精确 comment/Issue URL、Issue `updated_at`、标签、命令、actor、policy/config SHA、base/source SHA、Issue ref 和 lease expiry。
- build 只接受 allowlist 中的普通文本 add/modify，拒绝 rename/delete、symlink/submodule、可执行位、`.github/**` 和其他敏感路径，并按变更族执行可信测试。
- publish 只通过 App token 的 Git push 创建或推进 Writer ref，并固定使用 `--force-with-lease=refs/heads/agent/issue-N:<expected-old-sha>`；创建时 expected old 为空。lease 不匹配会由 Git 原子拒绝，不使用缺少 old-SHA 条件的 REST ref PATCH。每次 push 和 Draft PR create 之前仍再次执行现场 authorization；不会 review、approve、merge、ready、close、delete 或补偿性回滚。
- `/agent retry-write` 的 fix cycle 不接受 workflow input 或环境变量。preflight 从 exact head 上唯一、完整验证为 Writer App 所有的 open Draft PR 读取 canonical v2 receipt marker，并用 `AERIS_WRITER_PUBLIC_KEY` 验证 App 私钥生成的域隔离 RSA-SHA256 签名；marker 绑定上一条命令 comment、受信 lifecycle epoch、cycle、candidate SHA 与 commit SHA。旧 v1 marker 或 epoch 不匹配均无效。publish 在 mint token 前确认该公钥与 `AERIS_WRITER_PRIVATE_KEY` 匹配。首次 implement 为 cycle 0，后续不同 comment 只能递增到 1、2；同一 comment 重放和第 3 次 retry 均 fail closed。
- 每次判断 managed PR lifecycle 都必须完整读取该 PR 的 authoritative Issue Timeline。任意历史 `closed` 事件都会在受信 epoch 0 中永久 tombstone 对应 Issue，即使 PR 后来 reopened 也不得 retry、推进 ref 或更新 PR；timeline 无权限、API 失败、响应畸形或达到三页上限而截断时一律 fail closed。永久 tombstone 不通过 receipt 自报，也不会因 branch 删除、重建或旧 receipt 仍可验签而重置。
- mutation callback 完成命令、Issue、权限等现场读取后，还会最后读取并精确验证 main、issue ref 和完整 Draft PR ownership/lifecycle 快照；create 后立即复读 main/ref/PR 与 receipt marker。现有 Draft PR 的 title/body 没有可用 CAS，因此客户端不再发送 metadata PATCH：只有现场 title/body 已与目标值逐字节一致时才执行 read-only no-op 验证，任何人工或并发漂移均在 ref push 前 fail closed。
- 成功重放返回同一逻辑 receipt；模糊 create/push 只读对账，无法证明时返回 `residue`。当前 v2 retry receipt 会改变 PR body，因而 `/agent retry-write` 会以 `managed_pr_metadata_change_requires_cas` 在任何 ref mutation 前停止；在 GitHub 提供 metadata CAS 或迁移到其他可原子更新的权威 receipt 存储前，不得把 retry 描述为已启用。

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
- Actions audit/log 中 App token 只存在于 publish 进程，App installation 仍只包含本仓库。

canary 完成后立即将 `AERIS_WRITER_ENABLED=false`。保留 registry/policy enable 的决定必须经过单独安全评审；若任一验收项失败，保持变量关闭并人工核验 residue，禁止自动 close PR 或删除分支。
