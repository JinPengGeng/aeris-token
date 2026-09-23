# ADR: provider_api_keys 明文 `api_key` 列清理计划

- 日期：2026-09-21
- 状态：已接受（本批仅记录决策，**不在批次 I 删列**）
- 关联：Issue #208（🟡 中：`provider_catalog.rs:159,218` 遗留明文密钥列优先读取）
- 相关代码：
  - `crates/aether-data/adapters/postgres/src/provider_catalog.rs:161,220`（读取 `COALESCE(api_key, encrypted_key) AS api_key`）
  - `crates/aether-data/adapters/postgres/src/provider_catalog.rs:2304`（按密钥匹配 `COALESCE(api_key, encrypted_key) IS NOT DISTINCT FROM $3`）
  - Schema：`postgres/migrations/20260403000000_baseline.sql:478`（`encrypted_key text`）、`20260505000000_sync_core_export_columns.sql:26`

## 背景

`provider_api_keys` 表同时存在明文列 `api_key` 与密文列 `encrypted_key`。当前读取路径用 `COALESCE(api_key, encrypted_key)` 以明文列为优先来源。逐条代码核实结论：

1. 明文列仍是**优先**读取源；任何残留明文的行都绕过密文路径。
2. `encrypted_key` 在当前仓库中**没有任何写入点**（属上游遗产/死写入路径），即新写入的密钥不会进入该列，导致 COALESCE 的密文分支实际是“只读历史”。
3. 已确认现有读取/匹配调用方均为参数化绑定，无注入面；风险集中在数据敏感性与双轨状态的长期漂移。

## 决策

分三阶段收敛到单一密文列，禁止在本批或任何单一批次中直接 `DROP COLUMN`：

1. **回填与双写（预备批次，先于删列）**：引入 `encrypted_key` 的写入路径（或迁移期触发器/一次性回填脚本），保证全部活跃密钥均以密文形式存在；同时保留 `api_key` 明文列作为回滚逃生通道。
2. **灰度切换（读路径翻转）**：在回填覆盖率验证（活跃行 100% 有密文）后，将读取改为 `COALESCE(encrypted_key, api_key)` 并告警任何仍回落到明文分支的行；运行至少一个发布周期。
3. **删列（后续批次，单独 PR + 可回滚迁移）**：确认无读取路径依赖明文列后，用 `DROP COLUMN api_key` 迁移收尾，并在迁移前先导出/归档审计副本。

任何阶段发现活跃明文行，停止推进，回到阶段 1 修复回填。

## 影响

- 阶段 1 之前，本批保持 `COALESCE(api_key, encrypted_key)` 语义不变，避免“密文列为空时把可用密钥读成 NULL”的可用性回归。
- 迁移脚本必须幂等且带 checksum（遵循现有 migrations 纪律）。
- 删除明文列不可逆，必须与运维确认备份/回滚方案后方可执行。

## 进展

- 2026-09-23（阶段 1+2 合并实施）：数据层写入路径改为只写 `encrypted_key` 并将 `api_key` 置 NULL；读取/CAS/删除围栏统一改为 `COALESCE(encrypted_key, api_key)`（密文优先、明文 legacy fallback）；新增 maintenance 任务 `provider_credential_sweep`（启动即跑一次，此后每 5 分钟幂等巡检），把 `api_key` 列残余行搬入 `encrypted_key`（明文行先加密、已是密文的行原样搬移、无法用任何已配置 key 解密的密文行跳过并告警）。未删列——阶段 3 的 `DROP COLUMN` 仍为单独后续批次。
