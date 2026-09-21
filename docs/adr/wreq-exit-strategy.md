# ADR: wreq 退出/跟随策略评估

- 日期：2026-09-21
- 状态：已评估（纯文档，无代码变更）
- Refs: #226（第四轮评审 · 技术选型）

## 背景

wreq 6.0.0-rc.28（精确锁定，`=6.0.0-rc.28`）承载两条关键路径：

1. **TLS 指纹仿真**：`crates/aether-contracts/src/plan.rs` 中 `browser_wreq` 三后端之一；`apps/aether-gateway/src/execution_runtime/transport.rs` 硬编码 chrome100–145 共 38 个指纹模板。
2. **上游 WebSocket 代理**：`apps/aether-gateway/src/handlers/proxy/websocket/transport.rs` 使用 `wreq::ws`，**非 browser profile 也走 wreq**（`socket: wreq::ws::WebSocket`，`build_browser_wreq_client`）。

wreq 拉入 `boring-sys2 5.0.0-alpha.13`（alpha 版 BoringSSL 绑定，构建需 cmake + bindgen + perl），并自带 `wreq-util 3.0.0-rc.10` 预发布依赖。全仓预发布依赖仅 5 个且全部位于该链路。

## 事实核查（2026-09-21，crates.io 实测）

- **wreq 5.x 稳定线已不存在**：5.1.0、5.2.0、5.3.0 全部已被作者 **yank**（5.3.0 yanked 于 2025-07-30）。Issue 假设的"迁回 5.3 稳定线"选项事实上不可用。
- 作者（0x676e67，rquest 系硬分叉 reqwest）的版本演进：
  - 6.0.0-rc 线持续发布：rc.28（2026-02-11）→ rc.29（2026-06-03）→ rc.31（2026-08-18，rc.30 被 yank）。
  - 2026-08 起新开 0.15.x / 0.16.x 线（0.16.1 发布于 2026-08-27），README 定位转向 "privacy-aware"，最低 Rust 1.98，商用支持色彩加重。**版本号策略不稳定，是一个额外信号**。
- wreq 5.x README 即宣称支持 WebSocket Upgrade、TLS/JA3/JA4/HTTP2 指纹仿真、100+ 设备模板（由 wreq-util 维护）；6.0.0-rc 线能力只会更强。因此"指纹仿真需要 rc 才有的能力"这一假设不成立——5.x 已具备，问题只是 5.x 被 yank 了。

## 风险盘点

| 风险 | 现状 | 影响 |
|---|---|---|
| rc 依赖卡两条链路 | 指纹仿真 + 全部 WS 代理共用一个 rc 客户端 | 一次 yank/破坏性 rc 升级同时断两条链路 |
| BoringSSL alpha 绑定 | boring-sys2 5.0.0-alpha.13 | 构建链 cmake/perl/bindgen；与 openssl-sys 符号冲突风险（上游 README 自述） |
| 作者单点 | 单人维护、版本号反复（5.x→6.0-rc→0.15/0.16） | 供应连续性风险高于普通 rc 依赖 |
| 38 个硬编码指纹模板 | transport.rs 追版 Chrome | 长期维护税（已有，非本 ADR 解决范围） |

## 选项评估

### 选项 A：退出 wreq（全部迁走）

- 指纹仿真无成熟替代：crates.io 上没有维护良好且覆盖 TLS+HTTP/2 指纹的纯 rustls 方案，退出意味着放弃仿真能力或换到同样非主流的库。**不可行**。
- WS 代理迁回 tokio-tungstenite 可行（仓内 0.28 线已统一，且非 browser profile 的 WS 代理不需要指纹），但 `wreq::ws::WebSocket` 与该模块的流式接口耦合较深（`websocket/transport.rs` 全文件 9 处 `wreq::` 引用 + responses/* 会话模块），迁移需重写上游连接层并回归全部 WS 代理测试。

### 选项 B：冻结在当前 rc.28

- 已知可工作，但 rc 线继续演进，冻结越久后续跃迁（rc.28→rc.31 或新 0.16 线）的差异越大，实际是延迟决策。

### 选项 C：跟随（推荐，分两步）

1. **缩小爆炸半径**：把"非 browser profile 的 WS 代理"从 wreq 迁回 tokio-tungstenite + rustls，wreq 只保留指纹仿真一条链路。触发 wreq 故障时 WS 代理不受影响。
2. **跟随升级**：rc.31（或作者稳定下来的新线）发布后，在独立批次评估升级；每次升级只影响指纹仿真链路。

## 建议与触发条件

**建议：选项 C（跟随 + 收缩）。** 立即动作只有第 1 步（可作为独立 issue）；wreq 本体维持精确锁定。

触发重新评估的条件：

- 作者发布新的稳定大版本（非 yank 状态持续 ≥ 4 周）→ 启动迁移评估。
- 6.0.0-rc.28 出现安全公告或依赖冲突 → 优先做第 1 步收缩，再谈升级。
- 指纹上游（目标站）启用新 TLS 指纹校验导致现有 38 模板失效 → 升级 wreq/wreq-util 优先于手工补模板。
- WS 代理迁移完成（第 1 步落地）后，wreq 的退出成本降为"仅指纹仿真"，届时可每季度复查一次。

## 成本估算（WS 代理迁回 tokio-tungstenite）

- 改动面：`apps/aether-gateway/src/handlers/proxy/websocket/transport.rs` 上游连接建立与消息收发（`wreq::ws::WebSocket` → `tokio_tungstenite::WebSocketStream`），`websocket/responses/*` 的会话模块适配消息类型（`wreq::ws::Message` → `tungstenite::Message`），`execution_runtime/transport.rs` 中 browser 客户端构建仅保留指纹路径。
- 估计：300–500 行改动 + 全量 WS 代理回归（integration tests 已覆盖 `frontdoor/ai.rs` 等路径）。
- 收益：预发布依赖暴露面从"指纹+全部 WS 代理"降至"仅指纹仿真"；tokio-tungstenite 0.28 已在仓内统一，无新增依赖。

## 实施记录（批次 Q，2026-09-21，Closes #493）

### 已迁移（非指纹链路 → tokio-tungstenite 0.28）

- `apps/aether-gateway/src/handlers/proxy/websocket/transport.rs`：非 browser profile 的上游 WS 代理连接建立整体迁回 tokio-tungstenite。直连保留原 DNS 固定与私网/保留地址拒绝语义；HTTP 代理 `ws://` 用 absolute-form 转发（与原 wreq 客户端一致）、`wss://` 走 CONNECT；SOCKS 代理发送域名（远端 DNS，与归一化后的 `socks5h` 一致）。消息层引入传输中立的 `UpstreamWsMessage` / `UpstreamWebSocket`（Sink/Stream 双实现），消息类型映射、ping/pong、关闭帧、写超时与背压语义不变。
- `handlers/proxy/websocket/{responses,realtime,live}/**` 会话模块全部改用 `UpstreamWsMessage`/`UpstreamWebSocket`，不再 import `wreq::ws`。

### 保留 wreq 的链路（唯一保留理由 = TLS 指纹仿真）

- `execution_runtime/transport.rs::build_browser_wreq_client`：browser profile HTTP 链路（含 38 个指纹模板）。
- `handlers/proxy/websocket/transport.rs::connect_browser_upstream_websocket`：browser profile 的 WS 升级必须复用同一指纹客户端（wreq TLS 栈）。
- `execution_runtime/grok.rs`：grok.com 上游强制 browser_wreq profile，其 Imagine WS 复用指纹客户端。
- `bin/support/responses_ws_probe.rs`：探测工具，暂留 wreq（见"剩余触发条件"）。

### 依赖状态

- wreq 精确锁定 `=6.0.0-rc.28` 不变；`ws` feature 因 browser profile WS 升级仍需保留，未收窄。
- tokio-tungstenite 0.28 全仓单版本（workspace 根引用，`aether-gateway` 改为 `workspace = true`）；新增 `tokio-rustls 0.26` workspace 条目用于非指纹链路的 rustls 直连/CONNECT TLS。

### 剩余触发条件（承接上文，不变）

- 作者发布稳定大版本（非 yank ≥ 4 周）→ 评估整体退出。
- rc.28 安全公告/依赖冲突 → 先保指纹链路可用性预案。
- 指纹模板失效 → 升级 wreq/wreq-util 优先。
- 探测工具（responses_ws_probe）与集成测试中的 wreq 下游客户端可在后续批次顺手迁到 tokio-tungstenite，进一步缩小测试面依赖；不影响生产爆炸半径结论。
