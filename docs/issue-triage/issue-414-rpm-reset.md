# #414：RPM reset 查询移除整表清理

父项 [#215](https://github.com/JinPengGeng/aeris-token/issues/215)。基线为 `44bd242e15572e17cb808d2b9db213a443ab16cd`。

## 准确性与选择

`AppState::provider_key_rpm_reset_at` 原来在每个调度候选查询中取得共享 `StdMutex`，随后对全部 reset 标记执行 `HashMap::retain`；其开销与表容量相关，历史扩容后也可能放大。唯一生产写入来自管理员手动重置 provider key RPM，普通 AI 请求不增加记录。因此这是可局部移除的重复工作，优先级 P2；没有证据证明该表通常很大或它阻塞生产 1000 rps。

查询现在在原锁内只查目标 key，过期时仅删除目标；管理员写入保留原来的整表过期清理及覆盖写。没有改锁类型、调度 trait、数据格式或第三方依赖。读取从遍历表容量变为期望常数时间，但仍需取得共享锁。

定向复用考察了 `ExpiringMap` 的目标过期检查思路；其 `Instant` TTL 和容量驱逐不适合直接替换此处按调用方 Unix 秒比较、且会影响调度结果的标记。没有在缺少锁等待测量的情况下引入 DashMap 或修改锁语义。

## 时间、并发与保留边界

- `reset_at >= now.saturating_sub(60)` 保持有效；第 60 秒包含，第 61 秒过期。零附近的饱和减法、未来时间戳和最后一次写入覆盖语义不变。
- 查询复制时间戳、判断、删除在同一个 Mutex guard 中完成。旧标记的过期查询不会在释放锁之后删掉并发刷新的新标记；原 poison 行为不变。
- 未被再次查询的过期项可物理保留至下一次管理员写入；读不会增加条目，持续写入仍清理旧项，不以任意容量驱逐有效标记。HashMap 的历史分配容量仍可能保留，此切片不声称缩减 RSS。
- 标记仍为进程内状态，不跨节点传播，也不是 Redis RPM 计数器。在调用方时钟回拨时，尚未被查询或写入物理清理的旧标记可能重新落入窗口；不能声称所有非单调时间轨迹均与原查询整表清理完全一致。已经按目标过期删除的标记不会因后续回拨复活。
- 调度继续忽略观察时间不晚于 reset 的记录，并计入之后的新记录；没有改变 RPM、并发或其他准入规则。

## 验证与微基准

使用 Rust 1.95 和 Gateway CI 的 `RUST_MIN_STACK=16777216`。定向测试覆盖窗口、时间戳、覆盖写、多 key 隔离、闲置项写入清理及并发过期/刷新；真实候选选择覆盖 reset 前拒绝、reset 后旧/同秒记录被忽略、新记录继续拒绝。另复跑既有管理员 RPM reset Router 回归。

```sh
cargo test --locked -p aether-gateway --lib provider_rpm_reset -- --nocapture
cargo test --locked -p aether-gateway --lib gateway_resets_admin_key_rpm_locally_with_trusted_admin_principal
cargo test --locked -p aether-gateway --lib provider_rpm_reset_lookup_microbenchmark -- --ignored --nocapture --test-threads=1
cargo clippy --locked -p aether-gateway --all-targets -- -D warnings
cargo fmt --all --check
git diff --check
```

手工微基准用真实 `AppState` getter 与从旧主干精确复制的测试用 getter 比较，共用同一份已构造、预热的数据，交替顺序运行五轮并报告中位数。覆盖 0/1/64/4096 项、历史扩容到 4096 后只剩一项、每批 1/32/128 候选和 1/8 线程。构造与线程创建不计时，线程间 barrier 协调计入测量；小样本不可分辨的差异不解释为回归或收益。该 ignored 测试没有耗时门禁，debug 查询成本不能换算成生产 RPS 或端到端 p99。

2026-09-14 本地结果（macOS arm64 / Rust 1.95 debug 构建）：首次定向 4 项通过；整改后 state core、候选选择和既有管理员 Router 共 **84 passed / 0 failed / 1 个手工 benchmark ignored**，0.55 秒。单独执行 benchmark 为 **1 passed / 0 failed / 0 ignored**，9.89 秒；Gateway 全 targets Clippy、fmt、diff 检查通过。

五轮交替测量的代表性中位数如下；每行查询总数已列出，只在同一行比较两种实现。完整 30 行数据见 [CSV](fixtures/issue-414-rpm-reset-benchmark.csv)。

| 表状态 | 线程 | 查询数 | 原 getter，ms | 当前 getter，ms |
| --- | ---: | ---: | ---: | ---: |
| 空表 | 1 | 16384 | 1.192 | 0.674 |
| 64 项 | 1 | 16384 | 20.671 | 3.286 |
| 4096 项 | 1 | 1024 | 68.897 | 0.245 |
| 扩容后只剩一项 | 1 | 1024 | 19.393 | 0.227 |
| 4096 项 | 8 | 8192 | 571.597 | 4.080 |

上述样本使用每批 128 候选。实验说明移除了大表/历史容量扫描开销，未发现本组空表样本退化；不把小表的微秒差值、debug 放大比例或线程竞争结果推导为生产收益。`HashMap::capacity` 在删除后可能下调其公开报告值，CSV 如实保存实际容量，而不是假定仍等于原分配容量。

独立静态评审首次接受 getter、调度回归和文档边界，但发现并发测试把断言放在 barrier 之间，若断言失败会让对端永久等待。已将两线程观察值保存至汇合之后再断言，避免失败验收挂起；修订测试复跑通过，独立复审 **Accepted**。评审没有运行 Cargo，也没有把主线程计时当作独立测量。最初另一次评审进程因工具调用能力异常终止，没有计为有效评审；上述结论来自随后正常读取文件的独立评审及有界复审。

最终 head 必须通过四项 GitHub required checks；最新运行与合并状态以关联 PR 为准。

## 回滚及剩余

revert 本切片即可恢复原查询整表清理；无数据迁移。父 #215 仍开放，日志项已单独核实为既有主干修复，SSE、序列化与生产基准等未因此宣称完成。
