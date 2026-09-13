# Issue #403：nightly tunnel 制品交接

父项 #205；当前实现与离线验收已完成，仍待实际 cross 制品验证、独立评审和 hosted checks，PR 保持 Draft。

## 当前实现

- nightly 增加 Linux musl amd64/arm64 tunnel 构建，命名与 stable musl 制品一致。
- 包装阶段保留 gateway 的 SHA256SUMS，并为 tunnel 单独生成 SHA256SUMS.txt。
- 签名契约为 SHA256SUMS.txt.sig；公开信任输入通过 env 传递，复用生产严格 verifier。
- `source` 先验证 public trust；全未配置时明确省略 tunnel，保留 gateway nightly。部分/非法配置失败封闭。
- 签名位于独立的 `tunnel-sign` job，受 release Environment 审批保护；只有该步骤收到 private key。
- `package` 仅复制受保护 signing job 的已签名 bundle，并验证归档摘要；`publish` 在镜像/Release 写入前再次验签和验证摘要。
- package/publish/notify 的条件明确处理有意 skipped、签名失败和取消，不将缺配置当作 unsigned fallback。
- provenance、归档、manifest 和签名进入发布资产清单及 attestation。

主线程修正了初始草稿的 SHA256SUMS.sig 错误名称、musl 名字、shell 文本直接插入 vars 和
先推镜像再签名的顺序问题，并将受保护签名与既有 gateway 发布分离。源码、构建和 release 仍固定在同一个 workflow run commit。

## 配置与剩余验收

只读核验：仓库及 release Environment 没有 tunnel signing secret，仓库没有公开信任变量；
release Environment 允许 main 分支，但要求人工批准。全未配置时当前代码会明确省略 tunnel 制品；
只有 gateway nightly 继续，因此不需要为合并该代码提前上传私钥。

启用时，在 repository variables 配置 `AETHER_TUNNEL_RELEASE_KEY_ID` 以及
`AETHER_TUNNEL_RELEASE_PUBLIC_KEY`（原始 32 字节 Ed25519 公钥的标准 Base64）；轮换可用
`AETHER_TUNNEL_RELEASE_TRUST_KEYS` JSON trust set，必须与当前 signer ID/单公钥兼容。
将 PKCS8 PEM 私钥放入受保护 release Environment 的 `AETHER_TUNNEL_RELEASE_PRIVATE_KEY_PEM` secret。
公开信任变量必须是 repository 级，供无 secrets 的构建 job 使用；不要仅放到 release Environment。
公钥配置有效但缺私钥时签名 job 失败，整个已启用的发布路径中止。生成或替换正式密钥不属于离线测试。

继续步骤：

1. 验证实际 cross 构建的两个平台制品；现有 `Build aether-tunnel` workflow_dispatch 只构建、不发布，可用于同 SHA 的 musl 归档验证。
2. 独立审查修订后的完整 workflow、权限、环境和 artifact 信任边界；四项 required checks 后再考虑合并。

当前 updater 只支持 stable `tunnel-v*`。本项先限定 nightly 人工下载与验证，不扩展远程升级通道。
真实签名配置和 release Environment 审批保留各自边界；私钥不进仓库或日志。

## 验证与恢复

```sh
node --test .github/automation/test/nightly-tunnel-artifacts.test.mjs .github/automation/test/nightly-workflow-boundary.test.mjs .github/automation/test/tunnel-release-key-rotation.test.mjs
actionlint .github/workflows/nightly.yml
shellcheck .github/workflows/scripts/sign-nightly-tunnel.sh
git diff --check
```

2026-09-14：19 passed、0 failed/skipped（Rust 1.95、Node 24.13.1、OpenSSL；hosted CI 使用 Node 22）；actionlint、shellcheck 和 diff 检查通过。
5 个运行契约测试实际执行 workflow 的 preflight、包装、验签、发布与通知脚本；使用临时 Ed25519 key
和生产 Rust verifier 验证正常签名、坏签名、manifest 篡改、归档摘要不匹配、缺 key 和错误 signer。
测试真正创建/读取 tarball，验证 gateway-only 与 signed 两套精确资产清单、Release 参数和失败告警。
发布/通知中的 gh 由隔离 stub 接管，未访问真实 Release 或发送真实告警；fixture 二进制不是 cross 编译产物。

未执行真实 cross 构建、线上签名发布或最终独立评审。回滚恢复 gateway-only nightly，不涉及数据迁移。
