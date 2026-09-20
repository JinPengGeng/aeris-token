# Issue 220: residual supply-chain review

审计基于 `origin/main`（2026-09-13）。Issue #321、#346、#357 已覆盖签名发布清单、release key id 注入，以及 npm 依赖审计覆盖；本次只记录仍存在的边界，避免重复实现。

## 已闭环

- `install.sh` 下载 release archive 后读取同一 release 的 `SHA256SUMS`，要求 manifest 对目标归档只有一条有效记录，并用 `sha256sum` 或 `shasum` 比对后才解包。
- `apps/aether-tunnel/install.sh` 对 `SHA256SUMS.txt` 执行同等校验。
- 生产 `Dockerfile.app` 的 busybox 与 distroless 基础镜像均使用 immutable digest；Compose 中 postgres 与 redis 也使用 digest。
- release/nightly 生成 SHA256 清单并发布 Sigstore provenance，镜像推送使用构建输出 digest。

## 最终发布镜像门禁（2026-09-17）

正式 release 的唯一写入 job 仍是受 `release` Environment 保护的 `publish`；
审批 canary 保持只读。Buildx 只构建一次双架构 OCI archive，`push: false`，
不会先发布再扫描。`tools/ci/release_image_gate.py` 校验 OCI blob 的 SHA256、
descriptor size、manifest 和 config 中的 OS/architecture；必须恰好覆盖
`linux/amd64` 和 `linux/arm64`，缺失、重复、错误架构或未知 artifact 均失败。

Trivy 的 OCI 输入默认只选择第一个 descriptor，不能靠两次传入 `--platform`
证明双架构覆盖。因此脚本为每个已验证的 child manifest 建立单 descriptor
layout，并显式传入 `layout@sha256:child`。报告的 image config digest、OS、
architecture 和非空 OS package inventory 必须与原 archive 对应。
两个架构均执行，即使其中一个失败，已生成的报告也会保留。

策略位于 `.github/security/release-image-policy.json`：

- Trivy 固定 `0.74.0`，Linux amd64 下载包校验 SHA256；不新增浮动 action。
- HIGH/CRITICAL 阻断发布，包括尚无修复的漏洞；本门禁没有隐式 ignore、VEX 或
  `ignore-unfixed` 例外。需要例外时先单独评审有到期时间的策略，不修改报告绕过。
- 每个 release 仅从官方 `ghcr.io/aquasecurity/trivy-db:2` 解析一次 manifest digest，
  校验响应内容哈希后只按该 digest 下载到全新 cache。两个扫描共用同一数据库，
  `--skip-db-update` 防止扫描中漂移；网络失败不回退到旧 cache 或镜像源。
- 数据库 schema 必须为 2，`UpdatedAt` 距当前不超过 24 小时，不能超前超过
  5 分钟，且 `NextUpdate` 必须仍有效。不能用刚下载的 `DownloadedAt` 代替新鲜度。
- 扫描器故障、数据库过期、无包清单、假双架构报告与漏洞命中均失败关闭。

全部通过后，固定 digest 的 Skopeo `v1.22.0` 容器使用
`copy --all --preserve-digests` 将原 archive 复制到 metadata 生成的每个 tag。
每次复制的 digest 必须等于 Buildx 原 index digest；不能转码、重建或静默退回
单架构。GHCR/Docker Hub provenance 使用经过逐 tag 校验的同一个 index digest。
复制中断可能已写入部分 tag，脚本保留日志并停止，不把部分发布标记为成功。

`release-image-gate` artifact 无论成功或失败都保留 30 天，包含 policy、原 index、
archive/child/config digest 清单、scanner version、数据库 digest/metadata、每架构
JSON 报告与日志、最终发布 digest。大体积 OCI archive 和临时扫描 cache 不上传。

## 验证和剩余边界

`.github/automation/test/release-image-gate.test.mjs` 调用 Python 执行性 fixtures，
覆盖只有 arm64 有漏洞、缺/重架构、blob 损坏、descriptor/config 不符、数据库过期、
scanner 故障、两份 amd64 假覆盖、空报告、扫描后 archive 改变及发布 digest 不一致。
这些 fixtures 证明门禁控制流，不代表生产最终镜像已经无漏洞。

本地另用真实 Trivy 0.74.0 和当日 pinned DB 扫描包含 Debian package inventory 的
两个合成架构 OCI layout，验证了显式 child 选择、JSON 字段和报告校验兼容性。
真实 release archive、GitHub Environment 审批、所有 registry tag 复制与 provenance
仍须由首次受保护发布收集运行证据；没有通过的发布运行前，Issue #220 保持开放。

此门禁只适用于正式 release 镜像。nightly、独立 VSCodex 镜像、容器权限和既有 RSA
例外仍需各自验收。镜像内是编译后的 Rust 二进制和 frontend 产物，Trivy 无法证明
所有源码依赖均可被识别；现有 Cargo/npm advisory 门禁继续保留。
