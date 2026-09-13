# Issue #403：nightly tunnel 制品交接

父项 #205；实现、独立评审、离线签名与实际 cross 制品验收均已完成。
代码 head `2e017db2e392cd1b7427414c701bc4f8485e05cb` 的四项 required checks 全部通过；
最终合并仍须以最新 PR head 的门禁为准，正式签名启用保留下述配置边界。

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

代码已通过独立 workflow/权限/密钥/artifact 边界评审。若合并前更新主干涉及 tunnel 源码，
重新验证实际跨平台构建；文档状态更新本身不等于新二进制验证。

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

## 实际 cross 制品证据

[Build aether-tunnel run 34776139817](https://github.com/JinPengGeng/aeris-token/actions/runs/34776139817)
固定在上述代码 head，七个平台均构建成功；tag-only preflight、release、update-readme 均 skipped，
未创建 Release 或修改 README。本次分支 dispatch 复用 stable 构建入口，只作为编译/归档证据。

主线程下载两个 musl artifact，确认各 tar 仅包含可执行的 `aether-tunnel`，实际文件分别为
静态 x86-64 ELF 和静态 aarch64 ELF；不是单元测试的合成 binary。

| 归档 | tar.gz SHA-256 | Actions artifact ID |
| --- | --- | --- |
| aether-tunnel-linux-musl-amd64.tar.gz | `d2504ffb106ee5b8b26e0e9309fd668975332f5b0ccc8c4bc80e381648fcd0d3` | 10323531749 |
| aether-tunnel-linux-musl-arm64.tar.gz | `bc8cc0dbe06d4cb908e0b560de4680413791b70c1212d2512cd56d0efcd27fe8` | 10324041077 |

对这两个真实归档生成 manifest，使用一次性 Ed25519 key 执行本 PR 的签名脚本，随后再次执行
生产 Rust verifier 和 `sha256sum --strict -c SHA256SUMS.txt`，均通过；临时私钥已清理。
manifest SHA-256 为 `344a0dd57923d122d9d77f3731a9ccfda88f2391c9b640945909558d31930644`。
一次性测试 key 不属于正式信任链，不能将这次本地验签描述为已签名线上发布或二进制内置信任验收。

Actions artifacts 保留期为一天，之后可从记录的提交在本 fork 构建分支重新 dispatch；
本文件保留 run、提交、架构和摘要证据，交接不依赖原电脑的临时文件。
未执行线上签名发布，也未在本机运行 Linux 二进制。回滚恢复 gateway-only nightly，不涉及数据迁移。

### 合入 #402 后重新验证

PR #402 修改了 tunnel 服务配置迁移，因此在合入主干后对
`fa23cb537a177d38a1fb4026d8022c4dd4ccbc30` 重新 dispatch
[run 34776917186](https://github.com/JinPengGeng/aeris-token/actions/runs/34776917186)。
nightly 需要的两个 musl 平台均构建成功；下载后的 tar 内容、可执行位和静态 ELF 架构再次通过。

| 归档 | 新 tar.gz SHA-256 | Actions artifact ID |
| --- | --- | --- |
| aether-tunnel-linux-musl-amd64.tar.gz | `5c20d0353bf30e56361b73929213401a6b97012be0e049c93845e190e9def79c` | 10324196798 |
| aether-tunnel-linux-musl-arm64.tar.gz | `e62ea91b9e5a13bdaf565ed3e2e9a2baaa816738336d277e351aeafaadb8e587` | 10323688367 |

两个新归档再次通过一次性 key 签名、生产 verifier 和严格归档摘要验证；manifest SHA-256 为
`4d02fb1f42cbe6c0ff0802a4c515c6c08f333112a63e040b5a0a05070a1639eb`。测试私钥已清理。
后续合入 #400 仅新增交接文档，未改变上述已编译源码、构建配置或签名实现；最终 PR head
的 required checks 仍单独核验，不复用旧 head 状态。
