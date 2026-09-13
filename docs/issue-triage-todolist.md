# Issue 核验、排序与交付清单

当前交付入口：[完整动态 TODO](issue-triage/delivery-todo.md)。2026-09-13 已核验
fork main `1087c867e08aa517b28a5acfaf162c16ebf298e4`：当前实时为 44 个开放
Issue（21 P1 / 23 P2），15 个开放 PR（14 个启用受保护自动合并，#388 为唯一
Draft）。父子任务不能重复计数；以下历史统计保留但不代表当前状态。
所有 44 个开放 Issue 的 Project 卡均有 Status、Priority、Area、Risk、Size、Decision。

#276 已随 #386 的 merge commit 完成，重新 fetch 后验证指定上游 SHA 的祖先关系。
#362 资金数据层已合并，Gateway 完整接入仍在剩余验收中。#376 升级安全的
评审问题已修复并通过对应实测；#383 已合并，修复了
独立评审发现的 PostgreSQL 15 兼容性回归，15.19/17.11 真实迁移审计测试各通过
1 项且 0 ignored。#375 经范围复核关闭：release 公钥轮换不需要新增 Gateway
握手协议；#387 已实现实际 release 公钥集合与轮换/退役 fixtures，并补齐独立
helper 的 required audit 和 Dependabot，等待当前主分支检查。#379 已实现
默认零赠金并通过注册 HTTP 矩阵、前端测试和独立评审；显式促销和历史钱包保留，
#253 保持进行中。两个 PR 均已 Ready、开启自动合并。当前状态、验证链接、
剩余验收和排序以动态 TODO 与 GitHub 为准。

以下保留 2026-09-12 原始分诊证据及历史阶段记录，不代表当前未完成数量或状态。

## 2026-09-13 最新实时进展

- PR #387 已在 reviewed head `9c9880bf8673da2eadc1100ee038526bc6df0c61` 全部
  required checks 通过后合并，merge commit 为
  `c7563fd962ab2b08dc9136be6ff8f38702e1290d`；其 release signing-key overlap/
  retirement 与 verifier 依赖审计切片已交付，#205 的 recovery/nightly artifact
  残余仍开放。
- Draft PR #388（head `c4f6647213c9d3566ba220bb0e632b2d3cfdec9b`）保留
  prepared/dispatched/reconciliation_pending、`insufficient_quota` 和未结清
  recovery 记录，raw body/header 仍按策略过期；PG17.11、7 cleanup tests、12
  required live targets、Clippy 已通过，独立 review/当前 head checks 待完成。
  仅完成 usage-retention prerequisite，不宣称 Gateway 完整生命周期或已合并。
- #216 正在并行修复 selective Rust CI 回归：detector 输出 false 正常但执行 jobs
  丢失 `needs`/`if`，拟恢复保守过滤与 fail-closed gate；在 GitHub 生成 PR 前不填写
  PR 编号。
原始基线为 fork `main@12a1d265c090f2666e35ddbe7f13f5b842cf5ff5`。用户已授权
自主选择方案、补齐环境和社区协作配置，并要求全过程留档。只修改
[JinPengGeng/aeris-token](https://github.com/JinPengGeng/aeris-token)；误开的上游
#816 已关闭且没有代码合并，详细纠正记录见动态 TODO，不将历史误操作描述为从未发生。

## 统计与依据

最初 115 issues = 49 open + 66 closed。按当前代码重新打开 #92 后为 50/65；本轮关闭 #111/#200/#201/#202/#204 后为 **45 open + 70 closed**，后续拆出的实施 issue 另计。旧清单中“无 Project 权限”和直接复述旧 P0 的结论撤销。

- [安全/运行时审计](issue-triage/security-runtime-audit.md)：#205–#218，逐子项当前源码、已有测试与剩余范围。
- [运维/产品/治理审计](issue-triage/operations-governance-audit.md)：#220–#256，纠正 restore、checksum、digest、audit auth 等过期结论。
- [历史与tracker审计](issue-triage/history-trackers-audit.md)：全部历史关闭、重复、同步和观察记录。
- [Project审计](issue-triage/project-board-audit.md)：现有私有 Project 1 的实际字段与状态变更。

证据分为 `code_present → test_present → test_passed → runtime_verified`。引用已有测试不代表本轮执行通过；本地修复未合并不能关闭 issue。下列矩阵加历史审计覆盖全部115个历史issue，重复项不重复开发。

## 决策标准

P0 是当前可重现的核心故障/安全/资损；P1 是当前可达的可靠性或兼容性缺口、必要门禁；P2 是维护、性能、深防御或长期架构。S/M/L/XL 表示影响面与验证复杂度，不是工期承诺。旧报告标题中的 P0 不决定当前排期。

先交付小闭环；资金、公共接口、共享状态变更先做边界分析。用户已授权一般技术决策，无需反复确认工具安装和局部修复。历史钱款回补、收费策略变更、生产数据恢复或发布不能由旧审计推测执行。延期必须记录收益、依赖及重新进入条件，不能以旧“只跟随上游”评论一概拒绝本地改进。

owner 为维护者 JinPengGeng；主线程协调独立实现与评审。沿用 M0证据收敛、M1核心稳定性、M2协议与数据一致性、M3产品与运维增强；不虚构版本日期。

## 最短交付队列

| 顺序 | 工作与来源 | 收益/大小 | 实施与验收 | 状态 |
| --- | --- | --- | --- | --- |
| 1 | #92 watchdog 健康反馈 | 高/M | 复用HealthFailure/PoolStreamTimeout；retry/stop、双计时器、取消/终态互斥；定向测试、独立review、fork PR和CI | 本地实现，编译验证中 |
| 2 | #225 env名；#256 fork/贡献/安全入口；#229工具链与分级反馈 | 中高/S | clap对应名；fork安装/GHCR；贡献/许可文档；私密漏洞报告；链接/配置检查，文档PR | 本地完成；私密报告enabled=true已核验 |
| 3 | #211 TaskSupervisor drop | 中高/S | cancel并回收真实任务；drop、显式shutdown、panic/abort指标回归 | 本地实现，worker测试通过，待主审 |
| 4 | #206/#226 公式NaN/Inf | 高/S | 复用错误类型拒绝非法结果；除零/余数/溢出/合法边界，保留合法零值语义 | 独立修复中 |
| 5 | #255 mutation audit持久化 | 高/M | 核实tracing sink与repository全链；脱敏、失败/关闭语义；成功/失败变更重读测试 | 独立设计核验 |
| 6 | #211 OpenAI视频poll | 中高/S | in_progress映射；稀疏响应保留既有字段；重复poll/终态回归 | Ready |
| 7 | #216真实Postgres门禁；#220 advisory扫描 | 高/M | 实际执行钱包/结算ignored tests且数量非零；固定扫描版本与明确豁免 | Planned，workflow独立PR |
| 8 | #214/#217 readiness；#218生产profile | 高/M | readiness依赖超时/恢复，liveness不误杀；环境/cookie/CORS兼容 | Planned，需系统边界审查 |
| 9 | #206图片授权和header策略 | 高/M–L | 价格矩阵与授权同源；不可用时保守处理；采集前脱敏 | Planned，资金/安全独立设计 |
| 10 | #223 DLQ操作生命周期 | 高/L | 查询、有限保留、幂等redrive和权限；真实Redis重复重放/容量测试 | Planned |

## 运行时、安全、数据决策矩阵

混合报告只按当前残项开发；全部接受范围完成前不整体关闭。具体代码/测试位置见安全审计。

| Issue | 当前判断与残项 | 决定/优先级/大小 | 依赖及验收 |
| --- | --- | --- | --- |
| #92 | watchdog缺key/pool反馈；取消guard已有 | Develop/P0/M | retry/stop exact-once，不重复处理已开始终态 |
| #205 | redirect/body/private-target/downgrade已修；独立签名、Windows升级、regex/hash残项 | Planned/P1/L | trust root/轮换/旧客户端兼容ADR；无签名不是已证实攻击；性能项先基准 |
| #206 | enrichment写零已修；image gate、非有限公式、header断链成立；取消收费属策略 | Develop slice/P1/L | 先公式，再image授权矩阵；收费补偿策略单独记录 |
| #207 | 错误泄露/停机/channel/RPM默认已修；spawn ownership/lint残项 | Planned/P2/M | 共享supervisor后补任务接线，不全仓lint清扫 |
| #208 | NUMERIC CAST/LIKE/返利恢复已有；row decode、rollback可观测性残项 | Planned/P2/M | SQL类型回归；旧失败未插reward不能靠reconciliation回补，先dry-run与幂等证明 |
| #209 | URL/Debug泄露主项已修；quota时间/缓存/词表稳健性 | Deferred/P2/M | 具体provider可复现实例驱动，保持既有安全测试 |
| #210 | OAuth error已脱敏；Fernet salt属兼容；多项executor缺口无生产调用 | Deferred/P2/L | HTTPS/sequence契约分步加固，不替换存量加密格式 |
| #211 | 视频落盘已加密；drop、poll、retention、租约失效流终止残项 | Develop slices/P1/L | drop/poll先行；retention和客户端终止契约独立设计 |
| #212 | 容量非2xx统计已修；依赖双版本、端口/parser边界 | Deferred/P2/M | 实际兼容问题/flake证据后升级；共享测试契约 |
| #213 | 大模块与API文档是真实维护成本，旧行数非缺陷 | Deferred/P2/XL | 每次一个有边界与回归的模块，不做无行为收益大型搬迁 |
| #214 | shutdown/accept/idle已修；readyz静态和bulkhead默认残项 | Planned/P1/M | 依赖失败/恢复；liveness区分；容量默认须压测 |
| #215 | 日志已有NonBlocking；RPM reset锁/热路径env残项 | Deferred/P2/M | 基准证实热点后改缓存/锁；保留输出顺序契约 |
| #216 | VSCodex已有CI；live Postgres env/ignored入口缺口 | Planned/P1/M | 真实资金测试非零且故意破坏行为能使门禁失败 |
| #217 | readiness与#214共用；计费失败指标和告警资产 | Planned/P1/M | 低基数label，失败/恢复注入与告警验证 |
| #218 | JWT/ENCRYPTION_KEY/secrets/digest已修；compose仍development，Redis耐久默认需决策 | Planned/P1/M | 生产profile/cookie/CORS；usage队列耐久目标和恢复演练，当前env清单 |

## 运维、产品、架构与文档矩阵

| Issue | 当前判断 | 决定/优先级/大小 | 下一步及验收 |
| --- | --- | --- | --- |
| #220 | checksum/digest已修，扫描门禁缺口；root为运维权衡 | Planned/P1/M | 与#216共用扫描；降权须volume/升级测试，独立规划 |
| #221 | 依赖集中真实，一次搬迁缺收益证据 | Deferred/P2/XL | 当前依赖图与一个admin adapter垂直切片，endpoint tests不变 |
| #222 | 旧多库前提已变，扩展成本需真实样本 | Deferred/P2/XL | 一次provider/repository变更面测量后决定registry/dialect |
| #223 | restore binary/apply已有；DLQ无操作出口，重试/保留策略 | Planned/P1/L | DLQ闭环；隔离Postgres恢复演练，不操作生产恢复 |
| #224 | multi-node启动约束已有，参考部署/演练不足 | Planned/P1/M | 三实例共享Redis、identity与outage/cache窗口说明 |
| #225 | 旧七env大部已修，仅TCP_KEEPALIVE文档名残余；手册不足 | Develop/P1/M | env更正先交付；metrics/restore/multi-node手册独立验收 |
| #226 | RC依赖仍在，旧rustls结论未证；公式边界真实 | Develop formula/P2/M | 公式边界矩阵；cargo tree和clean build证据后决定transport升级 |
| #229 | 白名单重复、分级反馈/导航DX缺口 | Develop docs/P2/S | CONTRIBUTING真实命令；formula allowlist后续单一来源跨模块测试 |
| #235 | 财务/协议决策记录不足 | Planned/P2/M | ADR index、restore atomicity、tunnel version、usage retry/DLQ逐条代码证据 |
| #241 | with_redis_url静默no-op真实，不代表runtime仅内存 | Planned/P2/M | 删除或显式弃用builder、迁移调用；一致性/资金预留ADR |
| #247 | balance429/code歧义；通知基础设施已有 | Planned/P1/M | OpenAI/Claude兼容测试后决定契约；实际转换事件幂等/偏好测试 |
| #253 | gift/Turnstile属默认策略，不是已证实免单漏洞 | Planned policy/P1/L | 注册威胁模型、并发admit/settle仿真与告警；不追扣历史款或静默改存量收费 |
| #254 | formatter/model_not_found已修；API examples/错误矩阵不足 | Planned/P1/M | 与#247共用契约；chat/images quickstart对应实际fixture |
| #255 | route已鉴权；tracing有stdout/文件，缺DB桥接待完整设计 | Verify/design/P1/M | actor/action/target/status脱敏、写失败策略与DB重读测试；升级回滚手册独立 |
| #256 | fork身份/贡献/安全入口缺口真实；license不可自行改写 | Develop/P2/S | 文档PR；PVR启用核验；运行时install URL另列兼容残项 |

## 调度器存档与依赖

以下十项为Deferred，移除过期Ready/In progress/agent-ready。理由是跨HTTP/stream/WS契约、多节点状态和依赖尚未收敛，不是旧“只跟上游”政策禁止。存档分支作参考，不整体打捞旧数据库迁移。

| Issue | 现有/剩余 | 收益/大小/优先级 | 重新进入开发条件 |
| --- | --- | --- | --- |
| #1 | snapshot/page已有，十项umbrella | 间接/XL/P2 | 子项逐个验收，索引不作为重写任务 |
| #53 | sticky-key局部预算，无request-wide预算 | 高/XL/P1 | 总attempt/credential/provider/deadline同一扣减契约和exhaustion测试 |
| #51 | domain fail-closed，未生产接authoritative gate | 高/XL/P1 | #53后发送前quota/health/RPM/concurrency检查，依赖故障不发送 |
| #49 | 粗粒度classifier，无可信origin/replay contract | 高/XL/P1 | operation可重放性与预算/副作用语义先明确 |
| #46 | 记账lifecycle已有，无全传输ClientCommitted契约 | 高/XL/P1 | #49后；提交后不换candidate，HTTP/stream/WS联合测试 |
| #52 | 通用fencing已有，无专用HalfOpen lease | 多节点高/XL/P2 | 集群目标/Redis时间及失败策略，CAS/fencing/单探测测试 |
| #48 | history加密和API-key scope已有，无adapter capability | 中高/XL/P2 | 核实tenant隔离，native/hydrate/translate/unsupported声明与跨key测试 |
| #45 | candidate trace已有，缺budget/classifier/generation事件 | 中/L/P2 | #49/#51/#53语义稳定，可重建选择且不泄敏感payload |
| #44 | 应急domain gate已有，无ledger/授权/持久化/发送 | 场景相关/XL/P2 | 实际应急需求和审计/撤销/TTL验收；默认fail-closed |
| #47 | 存档验收依赖全部生产能力 | 高/L/P2 | 依赖实现后最后验证，不能只测桩件声称完成 |

## 同步、观察与历史关闭

| Issue | 决定 | 证据/后续 |
| --- | --- | --- |
| #200/#201/#202/#204 | 本轮completed关闭 | 告警SHA均origin/main祖先；PR203 merged；逐条评论保存命令/SHA |
| #111 | 本轮completed关闭 | 原目标完成；#157/#158、#92及patch-policy接续，无重复umbrella |
| #276 | Blocked/P1/M | 当前180个提交区间WalletCenter冲突；隔离分支语义解决，sync PR真merge，不squash |
| #157 | Deferred evaluation/P2/L | upstream732 merged、750 closed未merged、余项open；fork278已覆盖727区域，避免重复移植 |
| #158 | Deferred evaluation/P2/L | upstream736仍open；先证明授权粒度需求与#51兼容 |
| #179 | Planned/P2/M | Phase1–3已合，Phase0无完成证据；核验迁移/撤销后再关 |
| #268 | Planned/P1/M | 并发等待过早写Skipped；先核对attempt/candidate身份，保持单调终态，勿全面允许终态回转 |
| 其余65个原closed | 保持关闭 | 历史审计列25重复、14直接PR、17同步/流程、9完成/研究/信息记录；3个不可查旧SHA只作历史证据 |

## 社区流程和看板

沿用现有 [aeris-token Development Project 1](https://github.com/users/JinPengGeng/projects/1)，保留private与271个历史条目。project scope已补齐。Status扩展保留option IDs；Priority/Area/Risk/Size/Decision/owner/milestone按本表映射。未填的字段须实际补齐后才能称完成。

1. issue/本表记录触发、证据、收益、大小、依赖与验收，纠正旧主张。
2. 混合报告拆可测试切片；Ready仅指可动工，Deferred/Blocked不伪装In progress。
3. fork短分支，一项修复一个PR；只有完整范围交付才使用Closes。
4. 独立reviewer检查数据/并发/接口；主线程复核diff与测试数量，不以agent自述代替证据。
5. 远端required checks通过且基线最新；普通PR squash，sync PR merge并验证祖先。不得绕过保护或改绿测试。
6. 合并后复核main代码、issue关闭原因与Project；全部写入仅fork。

## 执行记录

- 核验全部115个历史issue，按当前代码重新打开#92，关闭5个过期/完成跟踪项。
- Project授权已完成，纠正错误Done与过期scheduler开发信号，补齐Area。
- 本fork Private Vulnerability Reporting由false启用并GET复核true；不虚构邮箱/SLA。
- 隔离Rust1.95.0位于`/tmp/aeris-rust.xKVBet`，不修改shell全局PATH；首次编译实际缺cmake后安装Homebrew cmake4.4.3。此前构建失败不是测试结果。
- 本地修复/独立review继续；最终PR记录实际命令、测试数、CI和残余风险。
