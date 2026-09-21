# Issue #215 残余性能项：failover 请求体缓存与 gzip 同步解码

状态：Accepted（2026-09-21；本轮不做实现，记录决策依据）

## 背景

Issue #215（性能评审）建议两项中等优先级优化：

1. **failover 重试缓存请求体序列化**：`transport.rs` 每次上游尝试对
   `plan.body.json_body` 做整棵 `Value` 深拷贝再 `to_vec`，多候选 failover 线性放大。
2. **gzip/br 同步解码 offload**：解压跑在 tokio worker 上（`transport.rs`）。

## 决策

本轮不做实现，仅记录决策。理由：

- **failover 体缓存**：收益取决于“长上下文 + 多候选重试”的请求占比。热路径已有
  Bytes 零拷贝切片与 singleflight 缓存；failover 重试路径每次多付一次深拷贝 +
  序列化，但在现有压测基线（S1–TPS）下不是 p99 的决定性因素（日志锁竞争与
  SSE 全量 Value 解析影响更大，见 #215 P1）。引入缓存需要处理 body 在不同候选间
  可能被改写（per-candidate header/body 变换）的正确性边界，回归风险大于收益。
- **gzip offload**：需要把解压迁移到 `spawn_blocking` 或专用线程池。响应体大小
  无个体上限（仅 256MB 全局信号量），大响应 offload 的排队延迟可能超过同步解码
  本身；且需要新增背压与取消语义。属于独立改造，不应搭车多节点批次。

若后续压测证据显示这两项进入 p99 前列，再单独立项实现。

## 复核入口

- failover 体拷贝：`apps/aether-gateway/src/.../transport.rs`（`plan.body.json_body.clone()` + `to_vec`）
- gzip/br 解码：同文件解压分支
- 判定依据：`tools/pressure/` S1–TPS 基线报告中的 p99 归因
