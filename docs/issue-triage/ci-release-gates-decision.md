# CI 与发布门禁复核：Issue #216 / #220

审计日期：2026-09-12  
审计基线：`d96a1b7c8`（fork `JinPengGeng/aeris-token`）  
范围：仅 fork；不修改上游 `fawney19/Aether`

## 结论

两个 Issue 的原始审查均有价值，但已经不是可直接按原文实现的单一任务。

* **#216 保持 Open，P1，拆分后进入开发规划。** 当前工作流已经包含 `aether-vscodex` 的独立 `VSCodex CI` job，且 `.github/change-filters.yml` 已声明 `vscodex: aether-vscodex/**`，因此 Issue 中“VSCodex 零门禁”的原结论已过期。仍然可复核且高价值的是 live PostgreSQL 测试发现/变量契约、未执行的并发/结算场景，以及将格式矩阵生成器 `--check` 接入 CI。build.rs 重编译和共享后端契约测试属于独立的性能/测试基础设施项目，不在本次门禁变更中处理。
* **#220 保持 Open，P1，范围收窄为依赖漏洞扫描门禁。** 安装器消费 `SHA256SUMS`、发布镜像 digest pin 已在当前 main 修复，不能重复实现。剩余 `cargo-audit`/`cargo-deny`、每个 npm lockfile 的审计和豁免治理需要单独设计，尤其要先固定工具版本、网络/数据库可用性、漏洞豁免格式和升级失败策略；不应在本审计中直接把可能阻塞所有 PR 的新门禁落地。

## 证据

### #216

* `.github/workflows/frontend-ci.yml` 当前有 `vscodex` job，执行根包 `npm test`、web `npm test`、extension `npm run check` 和 `npm run build`。
* `.github/change-filters.yml` 当前将 `aether-vscodex/**` 映射到 `vscodex`，并且 Frontend 聚合 job 将 `vscodex` 纳入 `Frontend CI / check`。
* `.github/workflows/rust-ci.yml` 只点名执行三个 PostgreSQL smoke test，并注入 `AETHER_TEST_POSTGRES_URL`；多个 adapter 的 ignored 测试读取 `AETHER_TEST_DATABASE_URL`，例如 `crates/aether-data/adapters/postgres/usage/tests.rs`、`settlement.rs`、`candidates.rs` 和 `video_tasks.rs`。两种变量不能互相替代，且 workflow 没有通用 `--ignored` 入口。
* `apps/aether-gateway/build.rs` 仍声明 `.git/HEAD` 变更触发重建。这是可测量的开发体验问题，但修复涉及版本戳缓存语义，不能作为低风险 CI 门禁补丁附带修改。
* `docs/api/generate_format_field_coverage.py` 提供确定性的 `--check`，当前工作流未调用；接入前需确认 Python 编码和生成器依赖在 runner 上稳定。

### #220

* `install.sh` 当前在解包前调用 `verify_release_checksum` 校验发布清单；该项已由仓库历史变更和 Issue #220 维护者复核确认。
* `Dockerfile.app` 当前基础镜像引用带 `@sha256:` digest；原“发布镜像无 digest”结论已过期。
* 当前 workflow 与依赖清单中没有 `cargo audit`、`cargo deny`、`osv-scanner` 或 `npm audit` 门禁。仓库存在以下 lockfile，未来扫描必须全部覆盖：根目录、`frontend`、`aether-vscodex`、`aether-vscodex/web`、`aether-vscodex/vscode-extension`、`.github/automation`。
* 审计不能把 `npm audit` 的网络服务结果当作可重复的确定性 required check；应先定义工具版本、锁文件范围、失败级别、暂时豁免的到期与 owner。

## 排序与验收建议

| 子任务 | 优先级 | 规模/风险 | 进入条件 | 验收证据 |
| --- | --- | --- | --- | --- |
| 统一 live PostgreSQL 测试变量，隔离数据库并分阶段启用 ignored 并发/结算测试（#216） | P1 | M / 中高 | 确认 services 生命周期、迁移和并行隔离 | CI 日志显示目标测试实际执行；失败时聚合门禁失败；运行时长基线 |
| 接入格式矩阵生成器 `--check`（#216） | P2 | S / 低 | 固定 `PYTHONUTF8=1` 和 Python 版本 | CI 在矩阵漂移时失败；无漂移时通过 |
| 设计并实现 Cargo/npm 漏洞扫描门禁（#220） | P1 | M / 高 | 固定扫描器版本、缓存/网络策略、豁免文件和升级窗口 | 六个 lockfile + Cargo.lock 均被扫描；可审计报告；过期豁免失败；不向不可信 PR 暴露 secrets |
| build.rs 版本戳重编译优化（#216） | P2 | M / 中 | 先有增量编译基准和版本语义决策 | 基准显示目标 crate 重编减少；版本输出和 dirty 状态回归测试 |
| memory/SQL 共享契约套件（#216） | P2 | L / 高 | 定义后端能力矩阵、迁移 fixture 和并发资源 | 同一场景在支持的后端执行并报告差异 |

## 社区流程决定

本复核只提交决策记录，不直接修改 `.github/workflows/**` 或引入新外部扫描服务。原因是这两项都改变 required-check 失败面，属于公共 CI 治理变更；应拆成独立 PR，并在 PR 描述中附运行时长、网络依赖、豁免规则、回滚方式和安全审查。实现时仍须经过：Issue 评论确认范围 → 短分支 → 测试/本地验证 → PR CI 与审查 → 维护者明确批准后合并 → Issue/Project 状态同步。

下一步由维护者或后续 agent 分别建立：

1. `#216-live-db-ci`：只处理变量统一、隔离和测试发现；
2. `#216-format-matrix-gate`：只处理生成器 `--check`；
3. `#220-dependency-audit-gate`：先提交设计与 PoC，再落地 required check。

