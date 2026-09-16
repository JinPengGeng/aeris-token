# Issue 218：JWT 与运行环境配置决策记录

日期：2026-09-12

## 复核结论

Issue 218 是复合问题，原报告中的 JWT 风险成立，但不能用一个改动宣称全部关闭。当前代码已经在请求处理时拒绝缺失、过短、公开开发值和 `.env.example` 占位值；此前服务启动阶段没有执行同一检查，因此错误配置可能在首次认证请求前看起来是健康的。`ENVIRONMENT` 的代码默认已改为 `production`；官方 Compose 部署路径显式注入 `production`，本地覆盖文件显式使用 `development`。

## 本次交付

- 服务网络启动（不含 healthcheck、data/migrate/backfill 子命令）现在执行 JWT 密钥 fail-fast 校验。
- 启动校验不使用测试专用密钥回退；缺失或不安全值直接返回配置错误。
- `docker-compose.yml`、`docker-compose.single-node.yml` 和 `docker-compose.release-local.yml` 默认注入 `ENVIRONMENT=production`。
- `docker-compose.local.yml` 保留 `ENVIRONMENT=development`，不改变 `make dev` 的本地工作流。
- `.env.example` 记录环境选择和生产默认姿态。

## 2026-09-16 CLI 与认证环境一致性

- 运行环境优先级为 `--environment`、`ENVIRONMENT`、`production`。有效值去除两端空白；空值使用 `production`。
- `main` 先解析参数，再将有效环境同步到认证 helpers 读取的 `ENVIRONMENT`，之后才启动 Tokio runtime 或日志后台线程。CORS 使用同一有效环境值。
- 生产环境默认 refresh cookie 为 `Secure; SameSite=None`；`development` 默认为无 `Secure` 且 `SameSite=Lax`。`AUTH_REFRESH_COOKIE_SECURE` 和 `AUTH_REFRESH_COOKIE_SAMESITE` 的显式设置继续优先于环境默认值。
- 解析测试覆盖 CLI 的 `production` / `development` 选择及 CORS，cookie 测试覆盖环境默认值与显式覆盖。测试不修改进程环境，避免影响并发测试。

## 未纳入本次范围

配置变量统一解析/完整文档、Docker 非 root 与资源限制、Redis Streams 持久化、数据库弱密码/TLS 默认值，以及真实部署迁移和密钥轮换，均需独立 Issue/PR、兼容性评估和运行验证。Issue 218 在这些工作完成前保持开放。

## 验证

- `git diff --check`：通过。
- Cargo 编译/测试：本机未安装 `cargo`，交由 required CI 执行。
- 未修改 upstream；未记录任何真实密钥。
