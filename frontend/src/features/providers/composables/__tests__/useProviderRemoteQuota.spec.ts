import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, type App } from 'vue'
import type { ActionResultResponse } from '@/api/providerOps'
import type { ProviderRemoteQuotaStatus } from '@/api/endpoints/types'
import { useProviderRemoteQuota } from '../useProviderRemoteQuota'

const api = vi.hoisted(() => ({
  syncRemoteQuota: vi.fn<(providerId: string) => Promise<ActionResultResponse>>(),
}))

vi.mock('@/api/providerOps', () => api)

let app: App | undefined
let root: HTMLDivElement | undefined

function mountComposable() {
  let composable!: ReturnType<typeof useProviderRemoteQuota>
  root = document.createElement('div')
  app = createApp({
    setup() {
      composable = useProviderRemoteQuota()
      return () => null
    },
  })
  app.mount(root)
  return composable
}

function actionResult(status: string, data: Record<string, unknown> | null = null): ActionResultResponse {
  return {
    status: status as ActionResultResponse['status'],
    action_type: 'sync_remote_quota',
    data,
    message: null,
    executed_at: '2026-09-07T00:00:00Z',
    response_time_ms: 5,
    cache_ttl_seconds: 0,
  }
}

function remoteQuota(overrides: Partial<ProviderRemoteQuotaStatus> = {}): ProviderRemoteQuotaStatus {
  return {
    enabled: true,
    group_id: '42',
    progress_endpoint: '/api/v1/subscriptions/progress',
    fetch_interval_seconds: 300,
    config_error: null,
    sync: null,
    ...overrides,
  }
}

beforeEach(() => {
  api.syncRemoteQuota.mockReset()
})

afterEach(() => {
  app?.unmount()
  app = undefined
  root?.remove()
  root = undefined
})

describe('useProviderRemoteQuota syncNow', () => {
  it('toggles syncing state and caches the last result', async () => {
    const composable = mountComposable()
    let resolveSync!: (value: ActionResultResponse) => void
    api.syncRemoteQuota.mockImplementationOnce(
      () => new Promise((resolve) => { resolveSync = resolve }),
    )

    const pending = composable.syncNow('provider-1')
    expect(composable.isSyncing('provider-1')).toBe(true)
    resolveSync(actionResult('success', { attempted: 1, applied: 1 }))
    const result = await pending

    expect(composable.isSyncing('provider-1')).toBe(false)
    expect(result.status).toBe('success')
    expect(composable.lastResult('provider-1')?.status).toBe('success')
    expect(composable.syncSummary(composable.lastResult('provider-1'))).toMatchObject({
      attempted: 1,
      applied: 1,
    })
  })

  it('clears syncing state when the call rejects', async () => {
    const composable = mountComposable()
    api.syncRemoteQuota.mockRejectedValueOnce(new Error('network down'))

    await expect(composable.syncNow('provider-1')).rejects.toThrow('network down')
    expect(composable.isSyncing('provider-1')).toBe(false)
  })
})

describe('useProviderRemoteQuota display helpers', () => {
  it('maps windows to display rows with labels and clamped percents', () => {
    const composable = mountComposable()
    const rows = composable.windowRows({
      sync_status: 'ok',
      windows: [
        { code: 'monthly', used_value: 120, limit_value: 100, used_ratio: 1.2, reset_at: '2030-01-31T00:00:00Z', is_exhausted: true },
        { code: 'daily', used_value: 1, limit_value: 10, used_ratio: 0.1 },
        { code: 'yearly', label: '自定义年' },
      ],
    })

    expect(rows).toHaveLength(3)
    expect(rows[0]).toMatchObject({ label: '月窗口', usedPercent: 100, exhausted: true })
    expect(rows[1]).toMatchObject({ label: '日窗口', usedPercent: 10, exhausted: false })
    expect(rows[2]).toMatchObject({ label: '自定义年', usedPercent: 0, used: null })
  })

  it('builds badge matrix for disabled/config error/pending/error/blocked/ok', () => {
    const composable = mountComposable()
    expect(composable.statusBadge(null)).toEqual({ text: '未启用', tone: 'muted' })
    expect(composable.statusBadge(remoteQuota({ enabled: false }))).toEqual({ text: '未启用', tone: 'muted' })
    expect(composable.statusBadge(remoteQuota({ config_error: 'bad' }))).toEqual({ text: '配置错误', tone: 'error' })
    expect(composable.statusBadge(remoteQuota())).toEqual({ text: '待同步', tone: 'muted' })
    expect(composable.statusBadge(remoteQuota({ sync: { sync_status: 'error' } }))).toEqual({ text: '同步失败', tone: 'error' })
    expect(composable.statusBadge(remoteQuota({ sync: { sync_status: 'ok', blocked: true } }))).toEqual({ text: '配额耗尽', tone: 'warn' })
    expect(composable.statusBadge(remoteQuota({ sync: { sync_status: 'ok', blocked: true, block_reason: 'subscription_invalid' } }))).toEqual({ text: '订阅失效', tone: 'warn' })
    expect(composable.statusBadge(remoteQuota({ sync: { sync_status: 'ok' } }))).toEqual({ text: '配额正常', tone: 'ok' })
  })

  it('syncSummary ignores payloads without a summary shape', () => {
    const composable = mountComposable()
    expect(composable.syncSummary(null)).toBeNull()
    expect(composable.syncSummary(actionResult('not_configured', null))).toBeNull()
    expect(composable.syncSummary(actionResult('success', { attempted: 1 }))).toMatchObject({ attempted: 1 })
  })
})
