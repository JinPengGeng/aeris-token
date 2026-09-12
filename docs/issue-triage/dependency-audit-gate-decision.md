# Dependency advisory gate decision (#220)

审计日期：2026-09-12  
适用仓库：`JinPengGeng/aeris-token` fork（不修改 upstream）

## 范围

Issue #220 中安装器 `SHA256SUMS` 消费和基础镜像 digest 已在当前 main
完成。本次只落实剩余的依赖漏洞检测：`Cargo.lock` 与仓库内每个 npm
`package-lock.json` 都必须被扫描。

覆盖的 npm 目录为：根目录、`frontend`、`aether-vscodex`、
`aether-vscodex/web`、`aether-vscodex/vscode-extension`、
`.github/automation`。

根目录当前只有用于锁定工具链依赖的 `package-lock.json`，没有
`package.json`；门禁因此对每个目录验证 lockfile 并使用
`npm audit --package-lock-only`，不把 `package.json` 存在作为前置条件。

## 门禁和工具版本

`.github/workflows/dependency-audit.yml` 在 pull request、main/master push、
手工触发和每周定时运行。Cargo 使用 Rust `1.95.0` 和固定的
`cargo-audit 0.22.2`；npm 使用 Node `22.14.0`，并将 registry 固定为
`https://registry.npmjs.org`，避免继承本地或 runner 镜像配置。

`cargo audit` 对 Cargo advisory database 中的漏洞失败；`cargo-audit 0.22.2`
通过 `rustsec 0.33`/`cvss 2.2` 支持当前 advisory database 使用的 CVSS v4.0
向量。安装阶段使用 `--locked` 锁定审计器自身的依赖，运行阶段由审计工具读取
仓库锁文件。任何 advisory 解析错误（包括未来不支持的 CVSS 版本）都会直接失败，
保持 fail-closed；npm 使用
`npm audit --package-lock-only --audit-level=high`：high/critical 漏洞失败，
moderate/low 仍会出现在审计输出中但不阻塞合并。网络、registry 或 advisory
数据库不可用同样失败（fail closed），因为无法证明依赖安全。

## 豁免与升级

当前没有漏洞豁免文件或静默忽略项。任何例外必须在单独 PR 中写明 advisory
编号、受影响路径、补救版本、owner、到期日期和风险接受理由，并由维护者
批准；不得通过向命令增加 `--ignore` 或降低门禁级别隐藏漏洞。升级扫描器或
Node 版本也需要单独变更并重新核验 required check 名称。

聚合检查 `Dependency Audit / check` 是 required check 的唯一门面；任一
Cargo 或 npm matrix job 失败时聚合检查失败。该工作流不读取 secrets，适合
不可信 pull request。
