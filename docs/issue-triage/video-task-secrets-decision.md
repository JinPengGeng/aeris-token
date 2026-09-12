# Issue #211 video-task secret handling decision

记录日期：2026-09-12

## 复核结论

Issue #211 的“upstream key/prompt 明文落盘”主风险已经由现有实现覆盖：`FileVideoTaskStore` 使用 Fernet v2 envelope 加密整个 registry，拒绝明文 registry，并在加载 legacy v1 envelope 后迁移到 v2。写入使用 sidecar lock、比较并替换（CAS）和临时文件原子 rename；新建 store、临时文件和 lock 文件使用 Unix `0600`。

本次补强处理两个仍可能导致敏感信息暴露的边界：

- 读取已有 store 时，若历史权限被放宽，启动阶段会将其收紧到 owner-only 后再读取；目录、symlink 等非 regular file 会被拒绝。
- `LocalVideoTaskPersistence`、`OpenAiVideoTaskSeed` 和 `GeminiVideoTaskSeed` 的 `Debug` 实现不再输出原始请求体、prompt、provider 错误、metadata 或 video URL，仅输出 `[redacted]` 占位符。

## 设计范围

本次变更保持“整库加密 + 诊断脱敏 + 文件权限自修复 + symlink 拒绝”的纵向切片，不扩展到 Issue #211 的其他工作包：registry 终态 retention/compaction、租约失效时的流式终止契约、cookie fallback、retry jitter，以及 OpenAI 状态/稀疏响应映射。

历史明文文件不会被自动读取或安全迁移。运维应隔离或删除该文件，并轮换可能已暴露的 provider 凭证和 prompt；重新启用持久化时必须配置有效的 `AETHER_GATEWAY_VIDEO_TASK_STORE_ENCRYPTION_KEY`（或对应部署注入项）。密钥不得写入仓库、日志或 issue/PR。

## 验收标准

- 明文 store、错误 encryption key、symlink/目录路径均 fail closed。
- 已存在的 group/world-readable store 在读取前收紧权限；写入后的 store、临时文件和 lock 文件保持 owner-only。
- 对含 synthetic key、prompt、error、metadata 和 URL 的 snapshot 调用 `Debug`，输出不包含原始敏感值。
- v1 -> v2 migration、CAS/lock 和现有 store 测试继续通过。
