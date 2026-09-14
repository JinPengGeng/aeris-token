# 2026-09-14 交付决策与任务基线

本记录承接 2026-09-14 的本地/远端同步复核和维护者确认。代码基线为
`origin/main=a42e3872cba604dfc2027872e7d7b3db8f70ce03`；本记录不把本地未推送
提交或仅有源码证据的事项写成已交付。

## 已确认的工作规则

- 代码必须经过 required checks 和 GPT-6-Astra 对抗性评审；两者通过后启用
  squash auto-merge。评审发现问题时先修复、重新评审，不绕过门禁。
- 主工作区保留用户已有的未跟踪脚本和现场修改。功能在独立分支/PR中推进，
  不直接改写 `main`。
- 不启动真实支付、生产 provider 或生产数据库。staging 和真实故障注入必须
  有隔离环境及回滚证据。

## 维护者已作出的决定

| 主题 | 决定 | 当前状态与边界 |
| --- | --- | --- |
| staging | 暂无独立 VPS，先记录方案，获得隔离节点后再验证 | 只完成拓扑/资源/故障注入清单；不能用本机 Compose 冒充生产验收 |
| 容器身份 | 直接迁移到 `10001:10001`，允许迁移旧卷属主 | PR #437；含迁移和回滚脚本、Compose/Dockerfile 更新；必须保留 Linux/WSL 实跑证据 |
| 无人值守升级 | 当前不启用；未来单独设计 | heartbeat 远程升级默认关闭；恢复前要有签名信任根、轮换、撤销和离线回滚方案 |
| 正式签名运营 | 当前个人及少量用户，不建设发布运营体系 | 手工升级和现有安全门保持；将来扩大用户面再立项 |
| RSA Marvin | 接受临时例外，截止 `2026-10-12` | 仅限 SQLx MySQL 可选依赖链；生产 PostgreSQL 路径不可达；保留精确 advisory、fail-closed 检查和到期复核 |
| #179 旧自动化 | 可以删除，但先清单、canary、独立 PR | 远端旧 workflow 已不存在；Writer policy/runtime、旧变量和 secret 仍需分批清理，secret 删除不可直接恢复 |
| #300/#206 负余额 | 采用方案 B：有限请求先调用，允许有限负数 | 不能解释为无限后付费；请求白名单、单次/日上限、负余额下限、告警和停止条件待固定 |
| #212 容量契约 | 参考 Sub2API 的分类，但不改变 Aether 调度策略 | 采用 HTTP 状态 + body/header reason 二级语义；区分 rate-limit、overload、model capacity、transport timeout |
| #217 可观测性 | 先采用上游调查结论，再做最小接入 | Aether 已有 metrics/health 路由；生产 scrape、告警 receiver、网络隔离和 dependency-aware readiness 仍是缺口 |
| #431 成本价目 | 先记录合同和数据来源，暂不改结算 | 需要脱敏供应商账单、币种/FX 来源和生效版本后才能计算毛利 |
| #316 历史兼容 | 不要求历史兼容，不做生产回补阻塞 | 保留 unknown 结论和只读审计工具；任何回补另需单独批准 |

## 当前交付队列

### P0：正在执行

1. **#436 CI 单实例基线**：修复 `aether_testkit` 导出缺失，重跑
   Integration Scenarios、Test 和汇总 check；通过后由 Astra 复核并自动合并。
2. **#437 non-root 容器迁移（#205 子项）**：完成 CI、Linux/WSL shell 验证、Astra
   复核后自动合并。卷迁移必须先备份、停 app、校验卷名，再执行 chown；回滚仅
   还原到文档声明的 root 属主。
3. **#435 退款通知偏好**：rebase 到当前 main，补足/确认异步通知路径测试，
   Astra 通过后自动合并；best-effort 不改变已提交退款结果。
4. **#253、#254、#255**：分别完成 rebase、聚焦测试、Astra 对抗评审和自动合并。
   父 Issue 仍保留未覆盖的完整验收范围。

### P1：下一批可并行

| 任务 | 交付物 | 依赖/停止条件 |
| --- | --- | --- |
| #211 | 凭证立即清理、任务元数据默认保留 7 天、lease 丢失的可重试 503 契约 | 逐 persistence/error path 盘点完成前不宣称全量关闭 |
| #212 | Aether/Sub2API 差异表、reason/reset/deadline 字段和调度不变证明 | 若发现公开 status/envelope 或调度语义差异，先维护者确认 |
| #217/#307 | 外置 Prometheus scrape/rules/receiver 的部署验收记录；保护 metrics/health | 无隔离 staging 时只提交配置/演练计划，不声称生产验证 |
| #300/#206 | 方案 B 的有限白名单与资金边界 ADR；再做 Gateway 接线 | 在上限、负余额 floor、告警/熔断值确认前不放开收费入口 |
| #303 | Marvin 例外到期提醒和 active-graph fail-closed 证据 | 到期前必须升级/移除 `rsa` 或重新评审例外 |
| #179 | 代码残留分批删除 PR；远端 Writer 环境最后处理 | 先确认无运行 workflow/开放 sync PR；secret 值无法回读 |

### P2：记录后续

- **隔离 staging**：准备单独 Docker Desktop/VM/VPS，使用独立 project、数据库、
  Redis、密钥和备份；按 `docs/operations/readiness-drill.md`、backup/restore
  和故障注入清单执行，完成后再把 #217/#223/#224 的部署状态前移。
- **远程升级签名**：选择 Sigstore/Artifact Attestations 或固定 Ed25519/minisign
  根，定义轮换、撤销、离线节点和 schema 兼容；在真实发布演练前保持关闭。
- **成本价目 (#431)**：由维护者提供脱敏供应商账单字段、价格版本、生效时间、
  币种和 FX 来源；没有这些输入只维护合同，不生成“已知毛利”。
- **历史审计 (#316)**：仅在未来有合规/运营需求时收集脱敏聚合证据；不自动写历史表。

## 已知验证限制

- 当前 Windows 工作区可以做 `git diff --check`、Compose config 和静态审查，
  但 Bash 脚本的完整执行必须在 Linux/WSL/CI 中完成。
- Rust 本地存在多份并发编译，部分 focused test 可能因资源/锁等待；这种结果只记
  为环境受限，不改写为代码失败或成功。
- `/_gateway/metrics`、`/_gateway/health`、`/readyz` 在 Rust 路由边界没有认证；
  只能内网绑定或由反代 ACL 保护。当前 `/readyz` 是进程/路由活性信号，不是依赖
  就绪证明。

## 需要在下一次变更前确认的最小输入

只有以下输入会阻塞对应实施，其余队列可继续推进：

1. #300 方案 B 的具体请求白名单（建议只含已审计的固定规格图片请求）、单次和
   自然日金额上限、允许的最低余额、告警阈值及自动停止条件。
2. staging 节点可用后，提供节点地址/访问方式、隔离数据库与 Redis、测试 provider
   凭证或本地 stub 选择；不要提供生产密钥。
3. #431 开始实现前，提供脱敏的供应商成本价目、币种和 FX 来源；缺失时保留
   `unknown`，不反推历史成本。
