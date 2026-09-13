# Issue #403：nightly tunnel 制品交接

父项 #205；当前是未完成 Draft，不满足自动合并条件。

## 已保存的草稿

- nightly 增加 Linux musl amd64/arm64 tunnel 构建，命名与 stable musl 制品一致。
- 包装阶段保留 gateway 的 SHA256SUMS，并为 tunnel 单独生成 SHA256SUMS.txt。
- 签名契约为 SHA256SUMS.txt.sig；公开信任输入通过 env 传递，复用生产严格 verifier。
- 签名/验证在镜像或 Release 发布之前；publish 受 release Environment 审批保护。
- provenance、归档、manifest 和签名进入发布资产清单及 attestation。

主线程修正了初始草稿的 SHA256SUMS.sig 错误名称、musl 名字、shell 文本直接插入 vars 和
先推镜像再签名的顺序问题。全部 14 个 nightly/rotation 边界测试通过；这仅证明接线约束，
不等于实际 cross 构建或密码学验签已通过。

## 当前阻塞与下一步

只读核验：仓库及 release Environment 没有 tunnel signing secret，仓库没有公开信任变量；
release Environment 允许 main 分支，但要求人工批准。目前草稿无条件要求 tunnel 签名，
因此若直接合并，会让缺配置的既有 gateway nightly 失败；必须先修正，不能把它合并为已完成。

建议下一实现切片：将 tunnel 签名放入独立受保护 job，并在 public trust 全部未配置时明确
跳过 tunnel 制品，保留 gateway nightly。部分配置无效、已启用后缺私钥或验签失败必须失败封闭；
不得发布 unsigned tunnel。需要仔细验证 skipped dependency 的 package/publish/notify 条件。
这是待实施方案，尚不是当前代码行为。

继续步骤：

1. 实现并测试未配置、部分配置、完整配置及签名失败四条路径，确保发布副作用顺序。
2. 用本地临时 Ed25519 key 执行真实签名/严格 verifier，测试坏签名、缺 key 和资产摘要不匹配。
3. 验证两个平台的归档结构、SHA manifest 和同一 source commit；不触发真实 Release 作为本地测试。
4. 独立审查修订后的完整 workflow、权限、环境和 artifact 信任边界；四项 required checks 后再考虑合并。

当前 updater 只支持 stable `tunnel-v*`。本项先限定 nightly 人工下载与验证，不扩展远程升级通道。
真实签名配置和 release Environment 审批保留各自边界；私钥不进仓库或日志。

## 验证与恢复

```sh
node --test .github/automation/test/nightly-workflow-boundary.test.mjs .github/automation/test/tunnel-release-key-rotation.test.mjs
git diff --check
```

2026-09-14：14 passed、0 failed/skipped；diff 检查通过。未执行 cross 构建、真实签名/验签、
线上发布或最终独立评审。回滚本草稿即可恢复现有 gateway-only nightly，不涉及数据迁移。
