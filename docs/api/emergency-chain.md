# 紧急链管理员 API

紧急链是管理员用于固定顺序同步模型测试的 operations-only 通道。它不接入
默认 scheduler，不做排序、排名或普通 fallback，也不是租户请求路由。

## 鉴权

两个路由都要求已认证的 administrator principal。使用 management token 时，
token 还必须具有 `admin:provider_query:admin` 权限。非管理员 principal 返回
`403`；撤销时 grant 不属于该 principal 也返回 `403`。

## 执行固定链

`POST /api/admin/provider-query/emergency-chain/execute`

请求体只定义 provider、模型和目标顺序：

```json
{
  "provider_id": "provider-1",
  "model": "gpt-4.1",
  "targets": [
    { "endpoint_id": "endpoint-1", "key_id": "key-1" },
    { "endpoint_id": "endpoint-2", "key_id": "key-2" }
  ]
}
```

`targets` 必须是 1--32 个不重复的 `endpoint_id` / `key_id` 对，数组顺序就是
发送顺序。缺少或无效 `provider_id`、`model`、targets、JSON，或不合法 ID，返回
`400`。当前 provider、endpoint、key、模型权限和 transport 不可用时不会开始
发送，并返回相应验证失败响应。

服务端生成 grant、request、nonce、fingerprint、chain hash 和时间字段；grant
固定存活五分钟。签发与审计记录原子持久化，随后在首次上游发送前一次性
consume。调用方不能重用 grant；consume 后即使进程在发送前中断，也不会被
Gateway 重放。

每次发送前都会强读取目标和 grant 状态。只允许同步标准文本 model test，固定
使用服务端测试消息和 `stream: false`。`408`、`429` 或 `5xx` 会前进到下一个
声明目标；成功立即停止，其他状态也停止。响应为 `200`，包含 `success`、
`grant_id`、`request_id`、`attempts`，成功时在 `data.response` 返回上游结果。

本端请求体不会把调用方额外 payload 当作自定义上游 body、prompt 或工具调用。
当前实现忽略未定义的额外字段，因此不要假定 `tools` 等字段必然得到 `400`。

## 撤销

`POST /api/admin/provider-query/emergency-chain/{grant_id}/revoke`

该路由不需要请求体。拥有该 grant 的管理员可撤销；成功或已撤销返回 `200`，
不存在的 grant 返回 `404`。撤销也写入审计记录。已 consume 的 grant 不能恢复，
撤销只阻止尚未发送的后续使用。

## 可用性与边界

未配置 PostgreSQL 持久化后端时，execute 和 revoke 返回 `503`，且不会创建或写入
grant。该路径只服务运维模型测试，不提供公共/tenant 紧急调度、通用 attempt
ledger 或任意提示词/工具执行能力。
