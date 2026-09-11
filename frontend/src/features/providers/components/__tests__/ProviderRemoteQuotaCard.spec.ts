import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, type App } from 'vue'

import ProviderRemoteQuotaCard from '@/features/providers/components/ProviderRemoteQuotaCard.vue'
import type { ProviderWithEndpointsSummary } from '@/api/endpoints/types'
import type { ActionResultResponse } from '@/api/providerOps'
import { createI18n } from '@/i18n'

const api = vi.hoisted(() => ({
  syncRemoteQuota: vi.fn<(providerId: string) => Promise<ActionResultResponse>>(),
}))

vi.mock('@/api/providerOps', () => api)

vi.mock('lucide-vue-next', async () => {
  const { defineComponent, h } = await import('vue')
  const Icon = defineComponent({
    name: 'IconStub',
    setup() {
      return () => h('span')
    },
  })
  return { RefreshCw: Icon }
})

function createProvider(
  overrides: Partial<ProviderWithEndpointsSummary> = {},
): ProviderWithEndpointsSummary {
  return {
    id: 'provider-1',
    name: 'Sub2API 中转',
    provider_priority: 0,
    keep_priority_on_conversion: false,
    enable_format_conversion: false,
    is_active: true,
    total_endpoints: 1,
    active_endpoints: 1,
    total_keys: 1,
    active_keys: 1,
    total_models: 0,
    active_models: 0,
    global_model_ids: [],
    avg_health_score: null,
    unhealthy_endpoints: 0,
    api_formats: ['openai:chat'],
    endpoint_health_details: [],
    ops_configured: true,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...overrides,
  }
}

function enabledRemoteQuota(): ProviderWithEndpointsSummary['ops_remote_quota'] {
  return {
    enabled: true,
    group_id: '42',
    progress_endpoint: '/api/v1/subscriptions/progress',
    fetch_interval_seconds: 300,
    config_error: null,
    sync: {
      sync_status: 'ok',
      synced_at: '2030-01-30T11:46:40Z',
      code: 'exhausted',
      exhausted: true,
      usage_ratio: 1,
      blocked: true,
      block_reason: 'window_exhausted',
      blocked_until: '2030-01-31T00:00:00Z',
      conservative_cooldown: false,
      subscription_id: '9',
      subscription_status: 'active',
      subscription_active: true,
      group_name: 'Pro',
      last_error: null,
      windows: [
        {
          code: 'monthly',
          label: '月',
          used_value: 100,
          limit_value: 100,
          used_ratio: 1,
          remaining_value: 0,
          reset_at: '2030-01-31T00:00:00Z',
          is_exhausted: true,
        },
      ],
    },
  }
}

function actionResult(status: string, message: string | null = null): ActionResultResponse {
  return {
    status: status as ActionResultResponse['status'],
    action_type: 'sync_remote_quota',
    data: { attempted: 1, applied: status === 'success' ? 1 : 0, blocked: 0, recovered: 1, skipped: 0, failed: status === 'success' ? 0 : 1, last_error: message },
    message,
    executed_at: '2026-09-07T00:00:00Z',
    response_time_ms: 5,
    cache_ttl_seconds: 0,
  }
}

let app: App | undefined
let root: HTMLDivElement | undefined

function mountCard(
  provider: ProviderWithEndpointsSummary,
  onSynced?: () => void,
) {
  root = document.createElement('div')
  document.body.appendChild(root)
  app = createApp(
    defineComponent({
      setup() {
        return () => h(ProviderRemoteQuotaCard, { provider, onSynced })
      },
    }),
  )
  app.use(createI18n())
  app.mount(root)
  return root
}

async function flush() {
  await Promise.resolve()
  await Promise.resolve()
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

describe('ProviderRemoteQuotaCard', () => {
  it('renders nothing when remote quota is disabled or missing', () => {
    const disabled = mountCard(createProvider({ ops_remote_quota: { enabled: false } }))
    expect(disabled.querySelector('[data-testid="provider-remote-quota-card"]')).toBeNull()

    const missing = mountCard(createProvider({ ops_remote_quota: undefined }))
    expect(missing.querySelector('[data-testid="provider-remote-quota-card"]')).toBeNull()
  })

  it('renders badge, windows and sync metadata when enabled', () => {
    const root = mountCard(createProvider({ ops_remote_quota: enabledRemoteQuota() }))

    expect(root.querySelector('[data-testid="provider-remote-quota-card"]')).not.toBeNull()
    expect(root.querySelector('[data-testid="provider-remote-quota-badge"]')?.textContent).toContain('配额耗尽')
    expect(root.querySelector('[data-testid="provider-quota-progress-row"]')).not.toBeNull()
    expect(root.querySelector('[data-testid="provider-remote-quota-subscription"]')?.textContent).toContain('Pro')
    expect(root.querySelector('[data-testid="provider-remote-quota-synced-at"]')?.textContent).toContain('最近同步')
    expect(root.querySelector('[data-testid="provider-remote-quota-blocked-until"]')?.textContent).toContain('熔断至')
  })

  it('shows config error instead of sync details when config is invalid', () => {
    const root = mountCard(createProvider({
      ops_remote_quota: { enabled: true, config_error: 'remote_quota.group_id 不能为空', sync: null },
    }))

    expect(root.querySelector('[data-testid="provider-remote-quota-config-error"]')?.textContent).toContain('group_id')
    expect(root.querySelector('[data-testid="provider-remote-quota-badge"]')?.textContent).toContain('配置错误')
  })

  it('calls the sync action and emits synced on success', async () => {
    api.syncRemoteQuota.mockResolvedValueOnce(actionResult('success', '同步完成：远程配额已恢复，已解除熔断'))
    const synced = vi.fn()
    const root = mountCard(createProvider({ ops_remote_quota: enabledRemoteQuota() }), synced)

    const button = root.querySelector<HTMLButtonElement>('[data-testid="provider-remote-quota-sync-button"]')
    expect(button).not.toBeNull()
    button!.click()
    expect(api.syncRemoteQuota).toHaveBeenCalledWith('provider-1')
    await flush()

    expect(synced).toHaveBeenCalledTimes(1)
    const feedback = root.querySelector('[data-testid="provider-remote-quota-action-feedback"]')
    expect(feedback?.textContent).toContain('已解除熔断')
  })

  it('surfaces failure payload and still emits synced for state refresh', async () => {
    api.syncRemoteQuota.mockResolvedValueOnce(actionResult('network_error', '网络错误'))
    const synced = vi.fn()
    const root = mountCard(createProvider({ ops_remote_quota: enabledRemoteQuota() }), synced)

    root.querySelector<HTMLButtonElement>('[data-testid="provider-remote-quota-sync-button"]')!.click()
    await flush()

    expect(synced).toHaveBeenCalledTimes(1)
    const feedback = root.querySelector('[data-testid="provider-remote-quota-action-feedback"]')
    expect(feedback?.textContent).toContain('网络错误')
  })

  it('shows transport error without emitting synced when the call throws', async () => {
    api.syncRemoteQuota.mockRejectedValueOnce(new Error('network down'))
    const synced = vi.fn()
    const root = mountCard(createProvider({ ops_remote_quota: enabledRemoteQuota() }), synced)

    root.querySelector<HTMLButtonElement>('[data-testid="provider-remote-quota-sync-button"]')!.click()
    await flush()

    expect(synced).not.toHaveBeenCalled()
    expect(root.querySelector('[data-testid="provider-remote-quota-action-feedback"]')?.textContent).toContain('网络错误')
  })

  it('renders 待同步 state when no sync has happened yet', () => {
    const root = mountCard(createProvider({
      ops_remote_quota: {
        enabled: true,
        group_id: '42',
        fetch_interval_seconds: 300,
        config_error: null,
        sync: null,
      },
    }))

    expect(root.querySelector('[data-testid="provider-remote-quota-badge"]')?.textContent).toContain('待同步')
    expect(root.querySelector('[data-testid="provider-remote-quota-synced-at"]')?.textContent).toContain('尚未同步')
  })
})
