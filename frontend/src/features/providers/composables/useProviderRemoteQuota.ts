import { ref } from 'vue'
import {
  syncRemoteQuota,
  type ActionResultResponse,
  type RemoteQuotaSyncSummary,
} from '@/api/providerOps'
import type {
  ProviderRemoteQuotaStatus,
  ProviderRemoteQuotaSyncStatus,
  ProviderRemoteQuotaWindow,
} from '@/api/endpoints/types'
import { log } from '@/utils/logger'

export interface RemoteQuotaWindowRow {
  key: string
  label: string
  used: number | null
  limit: number | null
  usedPercent: number
  resetAt: string | null
  exhausted: boolean
}

export type RemoteQuotaBadgeTone = 'ok' | 'warn' | 'error' | 'muted'

export interface RemoteQuotaBadge {
  text: string
  tone: RemoteQuotaBadgeTone
}

const WINDOW_LABELS: Record<string, string> = {
  daily: '日窗口',
  weekly: '周窗口',
  monthly: '月窗口',
}

/**
 * Sub2API 远程配额（PR-B 最小管理面）：手动同步动作 + 展示辅助。
 * 未启用 remote_quota 的 Provider 不会进入此 composable 的显示路径。
 */
export function useProviderRemoteQuota() {
  const syncing = ref<Record<string, boolean>>({})
  const lastResults = ref<Record<string, ActionResultResponse>>({})

  function isSyncing(providerId: string): boolean {
    return !!syncing.value[providerId]
  }

  function lastResult(providerId: string): ActionResultResponse | null {
    return lastResults.value[providerId] ?? null
  }

  function syncSummary(result: ActionResultResponse | null): RemoteQuotaSyncSummary | null {
    if (!result || typeof result.data !== 'object' || result.data === null) return null
    const data = result.data as Record<string, unknown>
    if (typeof data.attempted !== 'number') return null
    return data as unknown as RemoteQuotaSyncSummary
  }

  /** 触发一次手动同步；已完成（含失败 payload）即返回结果，传输层错误抛出 */
  async function syncNow(providerId: string): Promise<ActionResultResponse> {
    syncing.value[providerId] = true
    try {
      const result = await syncRemoteQuota(providerId)
      lastResults.value[providerId] = result
      return result
    } catch (error) {
      log.warn('[useProviderRemoteQuota] 同步远程配额失败', error)
      throw error
    } finally {
      syncing.value[providerId] = false
    }
  }

  function windowRows(sync: ProviderRemoteQuotaSyncStatus | null | undefined): RemoteQuotaWindowRow[] {
    const windows: ProviderRemoteQuotaWindow[] = Array.isArray(sync?.windows) ? sync.windows : []
    return windows.map((window) => {
      const usedRatio = typeof window.used_ratio === 'number' && Number.isFinite(window.used_ratio)
        ? window.used_ratio
        : null
      return {
        key: window.code,
        label: WINDOW_LABELS[window.code] ?? window.label ?? window.code,
        used: typeof window.used_value === 'number' ? window.used_value : null,
        limit: typeof window.limit_value === 'number' ? window.limit_value : null,
        usedPercent: usedRatio === null ? 0 : Math.min(Math.max(usedRatio * 100, 0), 100),
        resetAt: window.reset_at ?? null,
        exhausted: window.is_exhausted === true,
      }
    })
  }

  function statusBadge(
    remoteQuota: ProviderRemoteQuotaStatus | null | undefined,
  ): RemoteQuotaBadge {
    if (!remoteQuota?.enabled) {
      return { text: '未启用', tone: 'muted' }
    }
    if (remoteQuota.config_error) {
      return { text: '配置错误', tone: 'error' }
    }
    const sync = remoteQuota.sync
    if (!sync) {
      return { text: '待同步', tone: 'muted' }
    }
    if (sync.sync_status === 'error') {
      return { text: '同步失败', tone: 'error' }
    }
    if (sync.blocked) {
      return { text: sync.block_reason === 'subscription_invalid' ? '订阅失效' : '配额耗尽', tone: 'warn' }
    }
    return { text: '配额正常', tone: 'ok' }
  }

  function formatDateTime(iso: string | null | undefined): string {
    if (!iso) return '-'
    const date = new Date(iso)
    if (Number.isNaN(date.getTime())) return '-'
    return date.toLocaleString()
  }

  function formatAmount(value: number | null | undefined): string {
    if (value === null || value === undefined || !Number.isFinite(value)) return '-'
    return `$${value.toFixed(2)}`
  }

  return {
    syncing,
    isSyncing,
    lastResult,
    syncSummary,
    syncNow,
    windowRows,
    statusBadge,
    formatDateTime,
    formatAmount,
  }
}
