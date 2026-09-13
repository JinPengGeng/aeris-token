# Issue 220: residual supply-chain review

审计基于 `origin/main`（2026-09-13）。Issue #321、#346、#357 已覆盖签名发布清单、release key id 注入，以及 npm 依赖审计覆盖；本次只记录仍存在的边界，避免重复实现。

## 已闭环

- `install.sh` 下载 release archive 后读取同一 release 的 `SHA256SUMS`，要求 manifest 对目标归档只有一条有效记录，并用 `sha256sum` 或 `shasum` 比对后才解包。
- `apps/aether-tunnel/install.sh` 对 `SHA256SUMS.txt` 执行同等校验。
- 生产 `Dockerfile.app` 的 busybox 与 distroless 基础镜像均使用 immutable digest；Compose 中 postgres 与 redis 也使用 digest。
- release/nightly 生成 SHA256 清单并发布 Sigstore provenance，镜像推送使用构建输出 digest。

## 真实残余与决策

当前 CI 没有针对最终容器镜像的 CVE 扫描门禁（仅有 Rust/npm advisory 检查与 shell 供应链 fixtures）。这属于可见性和发布策略缺口，但新增扫描器必须先确定允许的漏洞数据库、误报处理、网络依赖和固定 action/image digest；直接接入一个浮动的第三方扫描 action 会降低已有 immutable-action 保证。

本次采取的最小闭环是：把 installer SHA256 校验和生产镜像 digest 作为回归契约，纳入 `tests/release_supply_chain_test.sh`。后续若要增加容器 CVE 门禁，应单独定义 severity 阈值、例外到期时间和扫描数据库镜像 digest，再修改 release gate；在这些决策落地前不把“扫描已通过”写入发布结论。

