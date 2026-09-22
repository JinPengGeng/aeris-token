# 决策记录：AETHER_LOG_FORMAT 默认值

状态：Amended（2026-09-22，批次 #217 监控栈交付）。compose 资产默认已切换为
`json`（仅 `docker-compose.yml` 的 `${AETHER_LOG_FORMAT:-json}`）；网关 CLI 默认值
仍为 `pretty`，JSON 仍可通过 CLI/环境变量显式开启。本记录其余部分保留原始决策
背景。

修订依据：`docker-compose.yml` 属于仓库随带的部署资产而非运行时默认值，切换其
占位默认值不满足"破坏性运行时行为变更"的否决条件（条件 3 针对 compose 资产的
同步要求由本批次满足：compose 与 `request-red-telemetry.md` 已同步）。历史部署
若依赖 pretty，设置 `AETHER_LOG_FORMAT=pretty` 即可无损回滚。

## 背景

第三轮运营评审（#217）指出：网关默认日志格式为 pretty（`main.rs` 的
`GatewayLogFormatArg` 默认值、`docker-compose.yml` 的 `${AETHER_LOG_FORMAT:-pretty}`），
不利于日志采集器按行解析 JSON。评审建议"compose 默认日志格式切 json"。

相关事实：

- 网关日志是结构化字段（`event_name` 485 个唯一值、`trace_id` 贯穿），warn/error 级别
  语义被压平（warn≈808 / error≈22），告警匹配必须以 `event_name` 为准，而非日志级别
  或文本格式。
- pretty 格式在多行事件（如带字段的 warn）上不可按行切分；JSON 格式可逐行解析。
- 变更默认值会影响所有现有部署的日志外观与采集管道（包括人工 `docker compose logs`
  排障体验）。

## 决策

**保持 `pretty` 为默认值，JSON 为显式可选项。**

理由：

1. **兼容性优先**：默认行为变更属于面向所有部署的破坏性调整；单维护者仓库缺少
   全量采集管道的迁移演练证据，不满足"行为改变需说明迁移条件"的 ADR 维护规则
   （见 `docs/adr/README.md`）。
2. **采集可行性不依赖默认值**：`docker-compose.yml` 已暴露
   `${AETHER_LOG_FORMAT:-pretty}`，部署方设置 `AETHER_LOG_FORMAT=json` 即可逐行解析；
   迁移步骤见 `request-red-telemetry.md` 的 collector migration 清单
   （canary → 解析校验 → 切换 → 回滚）。
3. **评审核心诉求已有替代抓手**：#217 的可观测性风险（无 RED 指标、无告警资产）已由
   请求级 RED 指标（`aether_gateway_request_total` / `request_errors_total` /
   `request_duration_ms_sum`）与 `docs/operations/prometheus/aether-alerts.yml`
   覆盖；日志格式不是告警链路的阻塞项。

## 何时重新评估（迁移条件）

满足以下全部条件时，可将默认值切换为 `json` 并标记本记录 Superseded：

1. 目标部署完成 canary 验证：JSON 逐行解析通过、字段完整性抽检通过；
2. 采集/排障双路径（采集器解析 + 人工 `docker compose logs` 可读性）均有回滚预案；
3. 变更随带 compose 与文档同步更新，并在 CHANGELOG 记为 breaking。

## 替代方案（已否决）

- **立即切换默认为 json**：收益仅是少一个环境变量；代价是所有未声明该变量的现有
  部署日志外观突变，且缺少迁移演练证据。否决。
- **双写（pretty + json 同时输出）**：现有日志栈不支持双格式输出，需改
  tracing subscriber 装配，超出批次 F 范围。否决。
