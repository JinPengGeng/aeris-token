# 数据契约胖 trait 拆分的受控试点：UserGroup 细粒度能力 trait

状态：Proposed（2026-09-21；试点代码已落地，全量推广仍待评审）

## 背景

Issue #222 评审指出：`aether-data/contracts` 的 repository trait 是"每加一个查询面 =
契约加方法 + N 套适配器实现"的乘法成本，其中 `UserReadRepository` 单个 trait 承载
74 个方法，覆盖用户导出、用户组、认证、OAuth 链接等多个内聚查询面。改进建议要求
把胖 trait 按查询面切成小 capability trait，但一刀切拆分 40 个 trait 的风险不可控。
本记录是**受控试点**：只选一个真实查询面（用户组）拆出 2 个细粒度 trait，量化成本，
为是否推广提供实证。

## 试点方案

在 `crates/aether-data/contracts/src/repository/users.rs` 新增：

- `UserGroupReadRepository`：6 个用户组读取方法
  （`list_user_groups` / `find_user_group_by_id` / `list_user_groups_by_ids` /
  `list_user_group_members` / `list_user_groups_for_user` /
  `list_user_group_memberships_by_user_ids`）。
- `UserGroupWriteRepository`：9 个用户组写入方法
  （`create_user_group` / `update_user_group` / `restore_user_group_if_matches` /
  `delete_user_group` / `replace_user_group_members` /
  `replace_user_group_members_with_audit` / `replace_user_groups_for_user` /
  `restore_user_groups_if_matches` / `add_user_to_group`）。

两个 trait 都通过 `impl<T: UserReadRepository + ?Sized> ... for T` 的 blanket
delegation 获得实现：**零适配器改动**（postgres / memory 均不需要新增 `impl` 块），
`UserReadRepository` 保持原样，所有既有调用点不受影响。

调用点适配（`apps/aether-gateway/src/data/state/auth.rs`，10 处）：把
`repository.list_user_groups()` 等裸调用改为
`UserGroupReadRepository::list_user_groups(&**repository)` 全限定调用，
消费端语义从"我需要一个 74 方法的胖 trait"收窄为"我只需要用户组查询面"。

## 量化结果

| 指标 | 数值 |
| --- | --- |
| 改动文件（数据侧） | 1（`contracts/src/repository/users.rs`，+约 230 行 trait 声明 + delegation） |
| 改动文件（gateway 调用侧） | 1（`data/state/auth.rs`，10 个调用点改全限定调用） |
| 适配器（postgres / memory）impl 块改动 | 0 |
| `UserReadRepository` 方法签名变化 | 0 |
| 编译影响 | `cargo check -p aether-data-contracts` 通过；gateway 调用点行为不变（同一方法体 delegation） |

## 结论与边界

- **方法成立**：blanket delegation 把"拆 trait"的适配器成本降到零，可以先建细粒度
  trait、再逐步迁移调用点，不需要大爆炸式重构。
- **本次刻意未做**：
  1. 把用户组方法**移出** `UserReadRepository`（需同步改 4+ 个实现与全部调用点，
     属于后续批次）；
  2. `dyn UserReadRepository` 存储字段类型收窄（`GatewayDataState.user_reader`
     仍是胖 trait 对象；调用点用全限定调用表达窄依赖）；
  3. 其它查询面（用户导出、OAuth 链接等）的 trait 拆分矩阵。
- **遗留风险**：trait 方法双份声明（胖 trait + 细粒度 trait）在迁移完成前长期并存，
  需要把"新调用点只依赖细粒度 trait"写进 code review 清单，否则债务不减反增。
