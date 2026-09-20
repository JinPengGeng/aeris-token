# #215：缓存 direct passthrough 模式配置

父项 [#215](https://github.com/JinPengGeng/aeris-token/issues/215)，基线 `aaaf6791addd7d6184810407336f684fbdd88bf7`。

`direct_passthrough_mode()` 位于流请求热路径，原先每次调用都读取并解析
`AETHER_GATEWAY_DIRECT_PASSTHROUGH_MODE`。模式是进程级启动配置，运行期间不需要动态变化；
现在沿用 transport 配置的 `LazyLock` 模式，在进程内首次访问时读取、解析并缓存单个
`DirectPassthroughMode`，后续请求只复制该枚举值。

解析契约保持不变：值会先去除首尾空白并转为 ASCII 小写；`legacy`、`pump` 和 `mpsc`
选择 legacy pump，其余值（包括未设置、空字符串、`inline` 和未知值）都选择 `Inline`。
测试覆盖大小写、空白、三个 legacy 别名及未知值回退。

该环境变量按启动配置管理。进程首次访问后修改环境变量不会刷新缓存；要应用新值必须
重启 gateway 进程。由于使用惰性初始化，精确读取时点是本进程首次进入相关请求路径，
其后语义与启动时固定配置一致。

本切片不修改 direct passthrough channel 容量、inline/legacy 实现、SSE 过滤、usage、
计费或流终止行为，也不增加动态配置刷新机制。无依赖或数据迁移；revert 本切片即可恢复
逐次读取环境变量。父 #215 的其他性能项继续独立跟踪。

验证命令：

```sh
cargo test --locked -p aether-gateway --lib direct_passthrough_mode_parser_is_case_insensitive_and_defaults_inline
cargo fmt --all --check
git diff --check
```
