# 管理 API 生命周期

本文记录当前 Rust Gateway 的管理面路由、认证方式和版本兼容约定。

## 当前路径与版本前缀

当前可执行管理面同时支持未版本化路径和 v1 别名。Gateway 将该前缀交给统一
代理处理器；控制面分类器对恰好以 `/api/admin/v1/` 开头的请求使用：

| 收到的路径 | 分类器的规范路径 |
| --- | --- |
| `/api/admin/v1/{path}` | `/api/admin/{path}` |
| `/api/admin/{path}` | `/api/admin/{path}` |

查询字符串保持原样；`/api/admin/v10/...` 和 `/api/admin/v1x/...` 不会被当作
v1。

分类完成后，公共请求上下文把 `request_path` 替换为规范路径，故本地处理器
按同一 `/api/admin/{path}` 匹配两种入口。认证签名、权限推导和查询字符串也
保持一致。客户端可使用 `/api/admin/...` 或 `/api/admin/v1/...`；两者都是
当前兼容表面。

各资源的实际契约由处理器和专题文档定义，例如 Provider Cost API 的
`/api/admin/billing/provider-costs/...` 见 [Provider Cost API](provider-costs.md)。通配符
不意味着所有将来 `{path}` 都有效；未分类并实现的路径不属于 API 契约。

## 认证与权限

`admin_proxy` 路由必须解析出管理员主体，否则返回 `401` 和
`admin authentication required`。当前本地方式为：

1. 管理员会话：`Authorization: Bearer <access token>` 必须为本地 access token；
   用户为活跃、未删除的 `admin` 或 `audit_admin`，且会话存在、未撤销、未过期，
   安全版本和客户端设备 ID 匹配。
2. Management Token：唯一的 `Authorization: Bearer <token>`，格式为 `ae-` 或
   旧格式 `ae_`；Token 必须活跃、未过期、通过 IP 规则，所属用户也须为活跃、
   未删除的管理角色。
3. 已验证可信认证上下文可传递内部管理员主体；外部入口会清除未受信任的此类头。

`admin` 可写；`audit_admin` 仅可读。后者在路由层被强制使用只读权限集，故
写入和需要 `:admin` 的操作会被拒绝。

Management Token 由 `admin:<scope>` 认证签名推导权限：`GET`、`HEAD`、
`OPTIONS` 为 `:read`，其他方法为 `:write`，同 scope 的 `:admin` 可满足两者。
暴露可用凭据、替换安全关键配置或高影响操作需要 `:admin`。缺少权限返回
`403`、`management token permission denied` 和 `required_permission`。

显式 permission 列表只授予列出的权限。无列表的旧 Token 使用冻结的
legacy-full 集；新增 scope 不会自动扩大它，且该集不包括 `management_tokens`。

## 兼容性与后续破坏变更

以下规则约束未来改动，不将任何当前端点标记为弃用，也不表示服务当前发送
`Deprecation`、`Sunset` 或版本协商头。

1. 已实现的 `/api/admin/...` 和对应 `/api/admin/v1/...` 路径是当前兼容表面。
   路径、方法、状态码、认证签名、权限及请求/响应语义的改变均属破坏变更。
2. 发布 v2 时，必须与 v1 并行提供可迁移的新契约，并覆盖认证、权限、成功和
   错误；在公告的停止支持日期前保留 v1。
3. 移除端点、改变上述契约或删除响应字段前，必须先提供并行新契约或兼容层，
   在发布说明中给出迁移和停止支持日期，并覆盖旧、新契约。
4. 可选/可忽略字段的新增应保持旧客户端可用；若改变默认行为、授权或副作用，
   仍按破坏变更处理。弃用必须另行发布并给出可验证的客户端信号；本文不是弃用通知。

## 源码依据

- `apps/aether-gateway/src/api/backend/admin.rs`：路由入口。
- `apps/aether-gateway/src/control/route/mod.rs` 与 `control/tests/admin_routing.rs`：v1 分类映射。
- `apps/aether-gateway/src/control/public.rs`：将处理器的 `request_path` 规范化为映射后的路径。
- `apps/aether-gateway/src/handlers/proxy/{mod,local}.rs`：主体提升、401、权限拒绝。
- `apps/aether-gateway/src/control/auth/resolution.rs`、`management_token_auth.rs`、`roles.rs`：认证。
- `apps/aether-gateway/src/control/management_token_permissions.rs`：权限和 legacy 集。
