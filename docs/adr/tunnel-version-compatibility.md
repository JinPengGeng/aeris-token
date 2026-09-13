# 隧道版本兼容与协商

状态：Accepted。日期：2026-09-13。源码基线与维护规则见 [ADR 索引](README.md)。
关联：Issue #235、#225。

## 背景与决定

隧道连接会同时承载多个请求。v3 的流窗口、信用更新与 reset/drain 需要双方在接收
业务流之前取得一致设置，否则较大的发送窗口会压垮较小的接收预算。接受现有 v3
`HELLO` / `SETTINGS` 握手及较小窗口协商，保留显式声明 v1/v2 的旧设置分支。
版本兼容必须从生产入口判断，不能只看单个解析 helper。

[公共 helper](../../crates/aether-gateway/tunnel/src/hub.rs)
`resolve_proxy_protocol_version` 对缺失/非法值回退 1，对可解析的高版本 clamp 至 3。
但当前 [WebSocket 入口 ws_proxy](../../apps/aether-gateway/src/tunnel/embedded/mod.rs)
在调用 helper 之前要求显式协议 header，解析为 `u8` 且位于 `1..=3`，并核对两次
解析结果；缺失、0、非法或超过 3 均拒绝。不能将 helper 的回退解释为线上自动降级。
节点 generation 与凭据认证仍须独立通过，版本号本身不提供权限。

## 当前入口矩阵

下表描述新连接的初始状态；不把初始版本解释为连接生命周期内不可变的安全属性。

| 声明/条件 | 入口与握手 | 流行为 |
| --- | --- | --- |
| header 缺失、非法、0、超过 3 | `ws_proxy` 拒绝，不依赖 helper 降级 | 不注册业务连接 |
| 显式 v1 | 完成当前节点/凭据校验；加密模式仍要求绑定的认证 HELLO；不要求 v3 SETTINGS | 使用本地默认设置，不启用按 v3 版本门控的流信用；取消发送 STREAM_ERROR |
| 显式 v2 | 与 v1 相同的 SETTINGS 分支和认证约束 | 在本文核验的流控制/取消路径中与 v1 相同；不推断其他历史实现的完整互操作性 |
| 显式 v3 | HELLO 版本匹配后读取合法 SETTINGS，回送协商值，随后注册连接 | 启用流信用与 WINDOW_UPDATE，取消发送 RESET_STREAM；支持 GOAWAY drain |

[代理握手](../../apps/aether-gateway/src/tunnel/embedded/proxy_conn.rs)
`handle_proxy_connection` 对加密连接先认证 HELLO 的控制帧类型、声明版本和
security session；v3 再由 `read_proxy_settings` 校验控制帧、顺序及参数，失败或超时
不注册连接。[SettingsPayload](../../crates/aether-contracts/src/tunnel.rs) 要求非零且
有界的窗口、有效的更新阈值和非零 drain deadline。协商窗口与 deadline 取双方较小值，
更新阈值不超过协商窗口的四分之一（最小 1 byte）。

[Gateway hub](../../apps/aether-gateway/src/tunnel/embedded/hub.rs) 的
`send_request_body_frame`、`flush_response_credit` 和 `cancel_local_stream` 按版本选择窗口与
取消帧。后续 SETTINGS 若在存在活动流时改变协商值会关闭连接，避免运行中切换预算。
hub 的后续 HELLO 分支仍会调用 `update_protocol_version` 并 clamp 至 `1..=3`；
它不是一轮完整的重新握手，本文不承诺所有运行期版本切换组合已获得互操作验收。
当前 agent 的 [连接代码](../../apps/aether-tunnel/src/tunnel/client.rs) 宣告协议 3，
发送 HELLO 与 SETTINGS；capabilities 列表描述功能，并非独立、任意组合的功能协商协议。

## 取舍、兼容与回滚

入口严格拒绝未知版本使版本与认证握手的解释保持一致，代价是依赖缺省 header 的
旧或自定义节点不能直接接入当前入口。显式 v1/v2 分支降低渐进升级成本，但不能获得
v3 的流窗口保障。这里的协议版本与 frame security/relay 签名中的 v1/v2 字样各自
独立，不应因相同数字推断兼容关系。

按 [Tunnel 升级说明](../../apps/aether-tunnel/README.md) 先升级 Gateway，再升级 agent。
与旧 Gateway 混用时保留默认窗口，不能依赖旧版实施当前协商规则。保留已验证的两端
版本、generation/凭据配置与部署记录；回滚先 drain，再切回实际验收过的配对版本。
重连仅恢复后续请求，不续传已输出的 SSE，也不保证已发送请求可安全重放。
不能通过删除版本 header 或关闭认证来修复不兼容。

## 可复核证据与未完成范围

- [helper 测试](../../crates/aether-gateway/tunnel/src/hub.rs)：
  `resolves_protocol_version_with_v1_fallback` 只证明 helper 的缺省与 v2 解析，
  不证明生产入口接受缺省值。
- [握手测试](../../apps/aether-gateway/src/tunnel/embedded/proxy_conn.rs)：
  `authenticated_proxy_hello_rejects_protocol_or_session_mismatch`、
  `authenticated_proxy_hello_rejects_non_encrypted_or_wrong_frame`。
- [设置测试](../../crates/aether-contracts/src/tunnel.rs)：
  `negotiation_bounds_window_updates_by_the_smaller_window`、`invalid_window_settings_are_rejected`。
- [流控测试](../../apps/aether-gateway/src/tunnel/embedded/flow_control_tests.rs)：
  `response_credit_is_not_returned_until_consumed`、
  `slow_stream_does_not_block_another_stream_on_the_same_connection`。

本次读取代码与测试，没有重新执行网络互操作测试。历史 v1/v2 二进制配对、跨平台
发布包和生产滚动升级没有因此获得完整验收。若要改变严格 header 校验或增加协议 4，
需另提兼容方案并补两端握手/流控回归；不把尚未实现的自动版本降级写成 Accepted。
维护责任由 contracts、Gateway tunnel 与 agent 的现有评审角色共同承担，部署配对由
部署责任人验收。参数的完整列表与漂移校验另见
[环境变量决策](../issue-triage/issue-225-tunnel-env-contract.md)。
