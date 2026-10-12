# 上游 open PR 吸收评估（2026-10-12）

- 评估对象：fawney19/Aether 全部 32 个 open PR（截至 2026-10-12，上游 tip `6bafef67`）
- 方法：剔除 patch-policy §4 已登记移植的 11 个（#634/#638/#640/#673/#706/#708/#714/#727/#729/#745/#607）与 #158 已跟踪的 #736 后，对剩余 24 个逐一核实——每个结论都在本仓代码对应位置验证过"我们是否已有/是否有此 bug"，非标题猜测
- 结论分类：建议吸收 9 个 / 可观察 6 个 / 不建议 9 个
- 执行状态：**三批次方案已批准（2026-10-12），暂缓执行**

## 1. 建议吸收（已批准，暂缓执行）

### 第一批（trivial × 4）

| 上游 PR | 内容 | 我们侧证据 | 价值 |
|---|---|---|---|
| #881 | stop sequences 恒序列化为数组（修 Go 强类型中转 400） | `crates/aether-ai/formats/src/protocol/canonical.rs:5340` 原样存在单元素压平逻辑 | 直接消除一类 API 400 |
| #623 | 全量系统导出纳入 admin 账号 | 我们的 export.rs 无此能力 | 备份恢复丢管理员是事故级缺陷 |
| #885 | DB 自动维护 3 张表扩到 105 张 | `apps/aether-gateway/src/maintenance/runtime.rs:194` 仍 3 张表，`usage_body_blobs` 等大表从不 VACUUM | 实盘磁盘健康 |
| #755 | 审计 body 内嵌 base64 图片压缩封顶（默认关） | 有截断机制无图片压缩，互补 | 防磁盘撑爆 |

### 第二批（medium，裁剪移植）

| 上游 PR | 内容 | 裁剪要点 |
|---|---|---|
| #588 | 失败请求展示候选级上游真实报错 | 与我们 #591 零文件冲突、互补（#591 只改表格显示，此 PR 补后端 payload） |
| #630 | Kiro 接受内联 system 消息 | 只取 `kiro/converter.rs` + 测试；剔除夹带的 `total_response_time_ms` u32→u64 改动 |
| #626 | embedding/rerank 放行 auth_type 覆盖 | 只取 conversion.rs 路由 + auth.rs 测试 + 前端放宽；剔除 kilo_free/opencode_free 部分 |
| #884 | 健康历史状态条改全窗口分段聚合 | 我们逐字存在该 bug（`catalog_helpers.rs:1094`）；需核对 .sql 架构守卫计数 |

### 第三批（large，单独立项）

| 上游 PR | 内容 | 说明 |
|---|---|---|
| #886 | 返利：规则快照 + 退款冲回 | **拆分吸收，不整移植**。"入口消失"bug 我们已自修（#589），奖励幂等已有（#481）；缺的是①付款时返利规则快照落库（历史订单防规则变更重算）②退款返利冲回预览（只扣赠款不动本金）。90+ 文件直插我们钱包/返利重定制区，UI 重构不跟 |

## 2. 不建议吸收（9 个）

| 上游 PR | 理由 |
|---|---|
| #633 Web 后台触发 Docker 更新 | 与容器非 root 加固（60aa2817）方向性冲突：Web 触发宿主机脚本执行把攻击面从容器内扩大到宿主编排；且 `docker-compose.yml`/`install.sh`/`update.sh` 全是 patch-policy 登记的永久 fork-owned 文件。价值为负 |
| #642 codex adapter 专线路由 | 基线陈旧（旧数据层路径已失效），移植等于重做；撞 `candidate_source.rs`（我们 #120 补丁区） |
| #647 免费 provider（opencode_free/kilo_free） | PR 用旧 crate 布局（`aether-provider-pool` 连字符目录），落后于上游重构；价值双面（免费容量 vs 侵蚀付费转化） |
| #608 用户备注/自动密码 | 基线严重陈旧（2026-05 迁移号 + 旧数据层路径）；remark 字段价值真实，但宁可按我们数据层规范自做 |
| #653 GLM coding plan 配额 | 引入自有配额持久化 + pool 块机制——正是 patch-policy 明确拒绝移植的设计；正确做法是按我们 remote_quota 接缝写 GLM 解析器 |
| #827 Key 级 Provider 限制 | 功能被 #736 超集覆盖；押注单一方案有 risk，跟 #736 为主、#827 为备 |
| #766 组级 Provider 限制 | 约 110 文件体量最大，与我们组倍率（#553）热区交叉最深；同上跟 #736 |
| #713 豆包视频端点+计费 | 三件事捆绑横扫全部计费热区，违反单补丁纪律；若视频计费 BUG 实测受损，单独提取计费修复 hunk |
| #735 API 参考手册 | 基于上游 main 逐路由核对，我们 admin 重定制区（billing/referrals/wallets）会失真，拿来即成错误文档；ops/database 通用章节可按篇摘取（待核实） |
| #840 流处理器 prefetch 溢出丢字节 | PR 已过时：上游 d4bc058c2（2026-09-27）换了方案（live 处理器直传 pump task），已在我们 main 中，我们已免疫；该 diff 已无法应用于当前上游 |
| #734 路由维度自动回切 | 不适用：我们无 `RoutingProfiles.vue`/`sortingScope`，前端路由 UI 已整体重构为 `features/routing/*` |

（表为 11 行含"不适用/已过时"两例，决策口径均为不吸收。）

## 3. 可观察（6 个，等上游合并自然带入）

| 上游 PR | 说明 |
|---|---|
| #887 usage PII 脱敏标记 | 与我们 SSE PII 还原（c0928143）互补的可观测性标记；等上游合并后 sync 回销，届时成本降为 trivial |
| #740 OrcaRouter provider | custom OpenAI 兼容 provider 已可达同等效果，无商业增益 |
| #656 批量编辑提供商 | 纯前端，与上游主线的 ProviderFormDialog 演进相邻，等 sync |
| #628 extra→xhigh 别名 | 非修复、无需求驱动 |
| #608（见上表） | 若上游另起新基线 PR 则重评 |
| #653（见上表） | 若上游合并其机制，sync 时按 fork-owned 语义剔除其持久化层，只保留解析语义参考 |

## 4. 上游已关闭 PR 核查（2026-10-12）

我们 patch-policy 登记"跟踪待合入"的 3 个上游 PR 实际已关闭，原因与含义：

| 上游 PR | 关闭者/时间 | 上游是否已有等价实现 | 对我们的含义 |
|---|---|---|---|
| #607 钱包套餐分组 | 作者 zhiqicloud 自行关闭，2026-09-23，无说明 | **没有**。上游无 `billing_source`；`plans.rs` 显式拒绝 `daily_quota.carry_over`（"is not supported"） | 回销无望，我们的 #186 实质转为 fork 长期持有功能 |
| #706 RUST_MIN_STACK | 作者 AAEE86 自行关闭，2026-09-15，无说明 | **部分**。上游 rust-ci 仅主测试 job 自带 1 处；我们的 #159 覆盖 integration-tests 4 个 bin | 补丁仍必要，转长期持有 |
| #745 跨格式协议回转 | 维护者 zhefox 关闭，2026-09-04；作者 stabey 8-23 rebase 后未获评审（CI 扯皮后无下文） | **没有**。上游无回转修复痕迹；相邻的 #733 同日被关闭 | 缺陷在上游仍在，我们的 #164 转长期持有 |

patch-policy §4 对应行的状态已随本文档同 PR 更新。

## 5. 待核实清单

1. #713 视频计费 BUG 是否影响我们实际在售模型（决定计费修复 hunk 是否单独提取）
2. 是否有 GLM Coding Plan 转售业务（决定是否按 remote_quota 接缝自写 GLM 解析器）
3. #840 残留：`sync_json_stream_bridge_active` 分支置 None 路径是否另有状态丢失
4. #884 的新增 SQL 与我们 usage 表定制列的兼容及 .sql 架构守卫计数
5. #734 同形态"删空后无法保存"缺陷在我们 `features/routing` UI 是否存在
6. #735 的 ops/playbook、database 通用章节是否值得按篇摘取
7. #736 上游评审动态（解冻 #158 前先观察）
