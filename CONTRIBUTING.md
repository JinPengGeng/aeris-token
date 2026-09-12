# 贡献指南

感谢你为 [JinPengGeng/aeris-token](https://github.com/JinPengGeng/aeris-token) 做出贡献。本仓库是 [fawney19/Aether](https://github.com/fawney19/Aether) 的 fork；请在本仓库创建 Issue 和 Pull Request。对上游可独立采用的小修复，维护者会按 [补丁与定制管理规范](docs/patch-policy.md) 评估并单独上游化。

## 开始前

- 使用 `rust-toolchain.toml` 和 `.mise.toml` 固定的 Rust `1.95.0`。安装 mise 后执行 `mise install`；也可让 rustup 按仓库中的 `rust-toolchain.toml` 选择 toolchain。
- 本地开发需要 Docker、Node.js 和 make。首次运行 `make dev` 前复制 `.env.example` 为 `.env`，并设置 `ADMIN_PASSWORD`；该命令会在需要时启动本地 Postgres 和 Redis。
- 不要提交 `.env`、访问令牌、数据库转储或生成的密钥。安全问题请遵循 [SECURITY.md](SECURITY.md)。

## 提交变更

1. 先在本仓库 Issue 中说明问题、范围和验收标准；关联已有 Issue 时，在 PR 描述中使用 `Closes #<编号>`。
2. 从 `main` 创建短生命周期分支，只完成一个可审查的改动。不要把上游同步、无关格式化或生成文件混入 PR。
3. 提交前按改动范围运行下列检查，并在 PR 中记录实际运行的命令和结果。
4. 所有 `main` 变更通过 PR 合并。分支保护、标签、必要检查和合并方式以 [开发工作流](docs/development-workflow.md) 为准；普通 PR 使用 squash merge，上游同步 PR 使用 merge commit。

## 本地检查

先按 [任务与模块导航](docs/module-map.md) 找到代码入口、架构约束和对应测试；模型别名等配置变更也在该文档中说明。

先运行与改动直接相关的最小命令，再在提交 PR 前补充适用的工作区检查：

```bash
cargo fmt --all --check
cargo check -p <package>
cargo clippy -p <package> --all-targets -- -D warnings
cargo test -p <package>
```

CI 的非网关/数据 Rust 测试使用 nextest。首次使用时安装 `cargo-nextest`，然后可执行：

```bash
cargo install cargo-nextest --locked
cargo nextest run --workspace --exclude aether-gateway --exclude aether-data --exclude aether-integration-tests
```

网关测试在 CI 中以 16 MiB Rust 栈运行；本地复现同一配置时使用：

```bash
RUST_MIN_STACK=16777216 cargo nextest run -p aether-gateway --lib
RUST_MIN_STACK=16777216 cargo nextest run -p aether-gateway --bins
```

数据层默认测试与真实 PostgreSQL 测试不同。`cargo nextest run -p aether-data` 在 CI 中设置 `AETHER_REQUIRE_LOCAL_POSTGRES_TESTS=1`，并要求本机可找到 PostgreSQL 服务端工具（`pg_config --bindir`）。标记为 ignored 的适配器测试需要隔离的 PostgreSQL 实例和 canonical `AETHER_TEST_DATABASE_URL`；CI 的 `Data DB Live (selected ignored tests)` job 使用一次性 PostgreSQL service、先执行迁移初始化，再由 `tools/ci/run_postgres_live_tests.sh` 串行点名运行高价值测试。`AETHER_TEST_POSTGRES_URL` 仅作为迁移 smoke 测试的兼容别名，并且必须与 canonical URL 完全一致。只在 disposable 数据库中运行该 harness，例如：

```bash
AETHER_TEST_DATABASE_URL=postgresql://USER:PASSWORD@HOST:5432/DB \
  bash tools/ci/run_postgres_live_tests.sh
```

该脚本不会启用未列入清单的 ignored 测试；job 失败会阻断 `Rust CI / check`。本地没有数据库时应保持 URL 未设置，让默认测试流程继续按原有行为运行。

前端改动遵循现有工作流：

```bash
cd frontend
npm ci
npm run lint:check
npm run type-check
npm run test:run
npm run build
```

`npm run lint` 会修改工作区；验证请使用不写入的 `npm run lint:check`。

## 署名与许可

提交即表示你有权按仓库的 [LICENSE](LICENSE) 贡献该内容。当前仓库没有 CLA 或 DCO 流程；不要在提交信息中伪造签署声明。请保留现有版权、许可和上游归属声明。
