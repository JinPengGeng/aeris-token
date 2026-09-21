# ADR: 依赖双版本统一 — 扫描结论与决策记录

- 日期：2026-09-21
- 状态：已扫描（本批次无依赖变更，纯决策记录）
- Refs: #226（第四轮评审 · 技术选型）
- 扫描命令：`cargo tree --workspace --format '{p}'` + 逐项 `cargo tree -i <pkg>@<ver> --workspace` 反向定位

## 结论先行

**本批次不做任何依赖变更。** 全仓双版本（及三版本）项全部是**传递依赖**，没有一处是工作区成员直接声明的版本不一致；直接依赖版本此前已收敛（thiserror 已统一为 2.x，tokio-tungstenite 已在主仓统一为 0.28）。在源头的间接依赖（sqlx 0.8.6、object_store 0.14.1、reqwest/hyper-rustls 0.27、tokio-tungstenite 0.24）不升级的前提下，任何"统一"都只能是 `[patch]` 级联，风险大于收益。

## 全量双版本清单与处置

| 包 | 并存版本 | 来源（反向定位） | 是否 provider/transport 相关 | 处置 |
|---|---|---|---|---|
| rand | 0.8.6 / 0.9.3 / 0.10.2 | 0.8←sqlx 0.8.6；0.9←tungstenite 0.28；0.10←object_store 0.14.1 | object_store 属 model-fetch 链路 | **留待 #222 残余批次**（随 object_store/sqlx 升级自然收敛）；其余记录为已知项 |
| rand_core | 0.6.4 / 0.9.5 | 分别随 rand 0.8 / 0.9 | 同上 | 同上 |
| webpki-roots | 0.26.11 / 1.0.6 | 0.26←工作区直接依赖（根 Cargo.toml、`apps/aether-tunnel`、`apps/aether-gateway`）+ sqlx-core；1.0←hyper-rustls 0.27（reqwest 0.12 链路） | tunnel 直接引用属避让区 | 工作区无法整体升到 1.0（tunnel 避让、sqlx 钉死 0.26）；**留待**，升 hyper-rustls 0.27→0.28 批次统一 |
| tokio-tungstenite / tungstenite | 0.24 / 0.28 | 0.24←`apps/aether-tunnel`（主仓已统一 0.28） | **是，避让区** | 已统一，不动（PR #487 在途覆盖 socket2 统一） |
| socket2 | 仅 0.6.3 | —（扫描确认已统一） | — | 无需处理 |
| thiserror | 仅 2.0.18 | —（1.x 已在 main 收敛） | — | 无需处理 |
| hashbrown | 0.14.5 / 0.15.5 / 0.16.1 / 0.17.1 | 0.14←dashmap 6.1.0；其余随 indexmap 等各代生态 | 否 | 纯传递，随 dashmap 7 升级收敛，记录为已知项 |
| getrandom | 0.3.4 / 0.4.2 | 0.3←rand 0.9 链；0.4←rand 0.10/uuid 1.22 | 否 | 随 rand 收敛，同上 |
| itertools | 0.13.0 / 0.15.0 | 0.13←aether-tunnel；0.15←object_store | tunnel 属避让区 | 留待 |
| block-buffer / crypto-common / digest / md-5 | 0.10 系 / 0.11–0.12 系 | 0.10←通用 crypto 生态；0.12←object_store 0.14 | object_store 属 #222 | 留待 #222 残余批次 |
| crossterm | 0.28 / 0.29 | 0.28←aether-tunnel；0.29←ratatui 0.30 | tunnel 属避让区 | 留待 |
| rustix | 多版本 | tar/xattr + crossterm 链 | 否 | 纯传递，已知项 |
| reqwest | 0.12.28 / 0.13.4 | 0.12←工作区直接依赖；0.13←object_store 0.14 | object_store 属 #222 | 留待 #222 残余批次（升 object_store 后 reqwest 0.13 链自然收敛） |
| foldhash | 单版本出现于锁文件 | — | — | 无需处理 |

## 决策记录

1. **不改 `[patch]`、不改 Cargo.lock 中 provider 相关条目**（批次规则）。所有涉及 `aether-provider/**`、`aether-model-fetch`、object_store、sqlx 的收敛点统一移交 #222 残余批次。
2. **避让区确认**：`apps/aether-tunnel` 的 tokio-tungstenite 0.24 / crossterm 0.28 / webpki-roots 0.26 维持现状，不属本批次。
3. **已知项接受**：hashbrown/getrandom/rustix/digest 系的多版本为生态自然分层（新旧大代际共存），无安全告警，统一成本（强制升级上游）大于收益，登记在案。

## 验证

- `cargo tree --workspace` 全量扫描（1832 行输出存档）。
- 逐项 `cargo tree -i <pkg>@<ver> --workspace` 确认每个并存版本的引入路径均为间接依赖。
- 本批次无 `Cargo.toml` / `Cargo.lock` 变更，无需 `cargo check`。
