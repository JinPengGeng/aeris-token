# Issue 218：JWT 与运行环境配置决策记录

日期：2026-09-12

## 复核结论

Issue 218 是复合问题，原报告中的 JWT 风险成立，但不能用一个改动宣称全部关闭。当前代码已经在请求处理时拒绝缺失、过短、公开开发值和 `.env.example` 占位值；此前服务启动阶段没有执行同一检查，因此错误配置可能在首次认证请求前看起来是健康的。`ENVIRONMENT` 的代码默认仍保留 `development`，以保持直接运行二进制和测试的本地开发兼容性；官方 Compose 部署路径改为显式注入 `production`，本地覆盖文件显式使用 `development`。

## 本次交付

- 服务网络启动（不含 healthcheck、data/migrate/backfill 子命令）现在执行 JWT 密钥 fail-fast 校验。
- 启动校验不使用测试专用密钥回退；缺失或不安全值直接返回配置错误。
- `docker-compose.yml`、`docker-compose.single-node.yml` 和 `docker-compose.release-local.yml` 默认注入 `ENVIRONMENT=production`。
- `docker-compose.local.yml` 保留 `ENVIRONMENT=development`，不改变 `make dev` 的本地工作流。
- `.env.example` 记录环境选择和生产默认姿态。

## 未纳入本次范围

配置变量统一解析/完整文档、Docker 非 root 与资源限制、Redis Streams 持久化、数据库弱密码/TLS 默认值，以及真实部署迁移和密钥轮换，均需独立 Issue/PR、兼容性评估和运行验证。Issue 218 在这些工作完成前保持开放。

## 验证

- `git diff --check`：通过。
- Cargo 编译/测试：本机未安装 `cargo`，交由 required CI 执行。
- 未修改 upstream；未记录任何真实密钥。
