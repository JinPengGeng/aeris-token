# Issue #205 service identity decision

记录日期：2026-09-13

## 决定

受管 `systemd` 和 OpenRC tunnel 服务必须使用专用的
`aether-tunnel` 系统账号运行。安装流程仍要求 root 创建或复用该账号，但在
生成服务定义前会读取账号 UID，并显式拒绝 UID `0`。因此，即使系统上的
`aether-tunnel` 名称被错误地映射到 root，安装也会失败，不会生成一个以 root
身份运行的服务。

主组必须是 `aether-tunnel`，且不得存在额外 supplementary groups；这些约束与
现有 systemd `User=`/`Group=` 和 OpenRC `supervise-daemon --user` 渲染保持一致。

## 验收

- `validate_service_uid` 的 fixture 覆盖普通非 root UID、UID `0` 和失败的 UID
  查询；UID `0` 明确返回错误。
- systemd/OpenRC 渲染测试继续要求专用用户和组，不允许 `root`。
- 本切片不改变容器 root 运行时或远程升级签名信任根；签名、轮换和恢复证据仍由
  #205 的其他子项跟踪。

## 限制

UID 检查依赖受信任的绝对路径 `id` 工具和系统账号数据库。安装主机若无法解析
`id`、`getent` 或服务组，流程会 fail closed；本地开发进程不受该 root-only 安装
路径影响。
