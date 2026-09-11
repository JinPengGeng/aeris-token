# #256 / #229 贡献者文档决策记录

日期：2026-09-12

范围：处理 #256 中 fork 身份、贡献入口、构建前置条件、许可证/商业边界、安全入口和变更记录的文档缺口；#229 中本地反馈分级入口的文档缺口。按用户已授权的社区协作配置，在本 fork 启用私密漏洞报告。不改变上游仓库、许可证、工作流或运行时代码。

## 决策

| 决策 | 依据 | 落地 |
| --- | --- | --- |
| 以本 fork 为唯一贡献与安装入口 | `origin` 指向 `JinPengGeng/aeris-token`，而 README 的 clone/install/nightly 链接仍指向上游 | README 改为 fork clone、raw installer、GHCR 和 dated nightly；上游页面、Star History、Logo 和归属保留并标明上游 |
| 固定 Rust 工具链 | `rust-toolchain.toml` 为 `1.95.0`，`.mise.toml` 原为 `latest` | `.mise.toml` 改为 `1.95.0`，CONTRIBUTING 说明 `mise install` / rustup 行为 |
| 记录真实的分级检查 | `.github/workflows/rust-ci.yml` 使用 package-level check/clippy、nextest、网关 `RUST_MIN_STACK=16777216`，并区分 data/真实 PostgreSQL 测试 | CONTRIBUTING 给出最小包检查、nextest、网关栈配置和隔离数据库环境变量；不新增虚构的 make target |
| 建立可用的私密安全通道 | `.github/ISSUE_TEMPLATE/config.yml` 已指向 advisories/new，但首次核验为未启用；用户授权补齐社区协作配置 | 在本 fork 启用 Private Vulnerability Reporting 并复核 `enabled: true`；SECURITY 提供真实私密入口，不添加邮箱、SLA 或赏金承诺 |
| 不改变商业授权或许可证 | LICENSE 是 `Aether 非商业开源许可证`，要求同条款分发且禁止再许可 | COMMERCIAL 仅转述许可文本与 fork 限制；边界不明或盈利使用要求版权所有人单独授权 |
| 不补写推测的发布历史 | 实际 fork release line 为 `aeris-token-v*` 和 `aeris-token-nightly-YYYYMMDD` | CHANGELOG 只链接本 fork Releases 并记录 tag 约定 |

## 验证

- `gh api repos/JinPengGeng/aeris-token/private-vulnerability-reporting`：2026-09-12 初查为 `false`；执行同路径 `PUT` 启用后，再次 GET 返回 `{"enabled":true}`。回滚可通过该设置的 DELETE 关闭；关闭后须同步更新安全入口说明。
- `gh api repos/JinPengGeng/aeris-token`：fork 为 `true`，parent 为 `fawney19/Aether`，default branch 为 `main`。
- `.github/workflows/release.yml` 仅接受 `aeris-token-v*`；`.github/workflows/nightly.yml` 生成 `aeris-token-nightly-YYYYMMDD`，并从 `github.repository` 计算 GHCR 镜像。
- `install.sh` 的默认 `REPO` 为 `JinPengGeng/aeris-token`、默认镜像为 `ghcr.io/jinpenggeng/aeris-token`，并按上述 tag 解析 Release。
- `LICENSE` 原文未修改；本次不创建 CLA、DCO、商业许可、邮箱或发布清单。
