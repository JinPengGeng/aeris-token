# Issue #398: service 配置迁移的链接安全边界

## 决策

服务安装在迁移旧配置权限前，从 `/` 的目录 FD 开始逐组件 `openat(O_DIRECTORY|O_NOFOLLOW)` 并通过 FD 校验所有祖先为 root 所有、不可由 group/other 写入。包括 sticky 目录在内的共享可写祖先均拒绝；不使用 `canonicalize` 或折叠 `..` 隐藏 symlink。最终配置必须为 root 所有、单硬链接 regular file，group/other 不可写。预检拒绝时没有权限修改或文件替换。

实际迁移重复完整 FD walk，并保持祖先、父目录和源文件 FD。源文件及复验打开同时设置 `O_NOFOLLOW|O_NONBLOCK`，防止 FIFO 替换造成挂起。复制复用配置加载器的 1 MiB 上限，并比较大小、mtime、ctime、设备/inode 和硬链接条件，拒绝已检测到的并发改动。源 inode 不作 chown/chmod：内容写入 `O_EXCL` 的 `0600` 临时 inode，设置目标身份与 `0640`、fsync 后，以同目录 `renameat` 提交。

只有文档约定的 `/etc/aether-tunnel` 专用配置目录可迁移为 `root:aether-tunnel 0750`。自定义配置父目录保持 owner/mode，必须已可被服务组或 other 遍历；不再为了安装修改 `/etc`、`/tmp`、`/root` 等任意父目录。祖先始终只校验不改权限。

`renameat` 是提交点。提交前错误清理临时文件；清理失败会再尝试限制为 `0600` 并明确报告。提交后专用目录权限或目录 fsync 失败，明确提示配置已经替换、安装中止或持久性未确认，不声称事务回滚。后续安装可重试迁移。

## 原因与兼容性

原实现先 `canonicalize(config_path)`，随后对 canonical 路径执行 `chown`/`chmod`，最后才检查链接。符号链接可使权限修改作用于目标，硬链接共享同一 inode。新方案保留 `/etc/aether-tunnel` 目录 `0700`、配置 `0600` 的合法安装迁移；自定义路径需提前具备安全的服务访问权限。`..`、symlink、可写祖先及不安全配置将明确拒绝。

威胁模型为不受信任的普通用户或服务身份；这些身份不能改写已校验的 root-owned 目录或配置。并发 root setup/TUI 保存和恶意 root 不受此切片支持。最终 inode 复验是检测措施，不是 compare-and-swap 保证；不把检查至 rename 的间隙误报为已消除。root 管理员必须串行执行配置写入与服务安装。

## 验收证据

- 最初草稿独立评审拒绝合并：只对最后父目录组件 NOFOLLOW、可能修改共享父目录、FIFO 挂起与缺成功路径测试。当前实现据此更换了路径处理和父目录策略。
- `cargo test -p aether-tunnel --bin aether-tunnel setup::service::tests -- --nocapture`：13 passed、0 failed、0 ignored（macOS arm64）。覆盖真正迁移的内容/新旧 inode/owner/mode/重复执行、自定义目录不变、sticky 拒绝、链接与 `..` 拒绝、父目录被 root 测试进程替换后仍只写 pinned inode、FIFO 及新文件替换拒绝、失败清理和超大配置拒绝。
- 测试从独立 fixture root 调用同一 FD walker，以当前用户身份运行；生产入口固定从 `/` 且只接受 UID 0，未为测试放宽生产检查。安全路径正向断言避免 macOS `/tmp` symlink 导致负例假绿。没有修改真实系统目录或真实服务账号。
- Rust 1.95 `cargo clippy -p aether-tunnel --all-targets -- -D warnings`、`cargo fmt --all -- --check`、`git diff --check` 通过。本机默认 Cargo/Rust 1.98 首次 Clippy 因已有其他 crate 的新 lint 失败，切换仓库指定 1.95 后通过，没有修改旁支代码。
- 系统调用 fsync/chown/unlink 失败的完整故障注入和真实 systemd/OpenRC 安装仍未执行，不作为已通过的证据。hosted checks 和修订后的最终独立复审仍待完成，交接 PR 保持 Draft。

该切片对应父 Issue #205 的子 Issue #398；远程升级签名、nightly 制品和 scheduler 设计债务不在本切片范围。
