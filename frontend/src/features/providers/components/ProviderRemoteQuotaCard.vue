<template>
  <Card
    v-if="remoteQuota?.enabled"
    class="p-4"
    data-testid="provider-remote-quota-card"
  >
    <div class="space-y-3">
      <div class="flex items-center justify-between">
        <h3 class="text-sm font-semibold">
          {{ legacyT('远程配额') }}
        </h3>
        <div class="flex items-center gap-2">
          <Badge
            :variant="badgeVariant"
            class="text-xs"
            data-testid="provider-remote-quota-badge"
          >
            {{ badge.text }}
          </Badge>
          <Button
            variant="outline"
            size="sm"
            class="h-7 px-2 text-xs"
            :disabled="syncing"
            data-testid="provider-remote-quota-sync-button"
            @click="handleSyncNow"
          >
            <RefreshCw
              class="w-3 h-3 mr-1"
              :class="{ 'animate-spin': syncing }"
            />
            {{ syncing ? legacyT('同步中') : legacyT('立即同步') }}
          </Button>
        </div>
      </div>

      <div
        v-if="remoteQuota.config_error"
        class="text-xs text-red-600 dark:text-red-400"
        data-testid="provider-remote-quota-config-error"
      >
        {{ remoteQuota.config_error }}
      </div>

      <template v-else>
        <div
          v-if="rows.length > 0"
          class="grid gap-3"
          :class="rows.length > 1 ? 'grid-cols-2 sm:grid-cols-3' : 'grid-cols-1'"
        >
          <ProviderQuotaProgressRow
            v-for="row in rows"
            :key="row.key"
            :label="`${legacyT(row.label)} ${formatAmount(row.used)} / ${formatAmount(row.limit)}`"
            :used-percent="row.usedPercent"
            :meter-class="row.exhausted ? 'text-red-600 dark:text-red-400' : 'text-muted-foreground'"
            :bar-class="row.exhausted ? 'bg-red-500' : row.usedPercent >= 70 ? 'bg-yellow-500' : 'bg-green-500'"
            :reset-text="row.resetAt ? `${legacyT('重置时间')}: ${formatDateTime(row.resetAt)}` : null"
            :title="row.exhausted ? legacyT('窗口已耗尽') : null"
          />
        </div>

        <div class="space-y-1 text-xs text-muted-foreground">
          <div v-if="sync?.group_name || sync?.subscription_status">
            <span data-testid="provider-remote-quota-subscription">
              {{ subscriptionText }}
            </span>
          </div>
          <div data-testid="provider-remote-quota-synced-at">
            {{ legacyT('最近同步') }}: {{ sync ? formatDateTime(sync.synced_at) : legacyT('尚未同步') }}
          </div>
          <div
            v-if="sync?.sync_status === 'error' && sync.last_error"
            class="text-red-600 dark:text-red-400"
            data-testid="provider-remote-quota-last-error"
          >
            {{ legacyT('最近错误') }}: {{ sync.last_error }}
            <span v-if="sync.last_error_at">({{ formatDateTime(sync.last_error_at) }})</span>
          </div>
          <div
            v-if="sync?.blocked && sync.blocked_until"
            data-testid="provider-remote-quota-blocked-until"
          >
            {{ legacyT('熔断至') }}: {{ formatDateTime(sync.blocked_until) }}
            <span v-if="sync.conservative_cooldown">({{ legacyT('保守值') }})</span>
          </div>
        </div>
      </template>

      <div
        v-if="actionFeedback"
        class="text-xs"
        :class="actionFeedback.ok ? 'text-green-600 dark:text-green-400' : 'text-red-600 dark:text-red-400'"
        data-testid="provider-remote-quota-action-feedback"
      >
        {{ actionFeedback.text }}
      </div>
    </div>
  </Card>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'
import { RefreshCw } from 'lucide-vue-next'
import Badge from '@/components/ui/badge.vue'
import Button from '@/components/ui/button.vue'
import Card from '@/components/ui/card.vue'
import ProviderQuotaProgressRow from '@/features/providers/components/ProviderQuotaProgressRow.vue'
import { useI18n } from '@/i18n'
import { useProviderRemoteQuota } from '@/features/providers/composables/useProviderRemoteQuota'
import type { ProviderWithEndpointsSummary } from '@/api/endpoints/types'

const props = defineProps<{
  provider: ProviderWithEndpointsSummary
}>()

const emit = defineEmits<{
  (e: 'synced'): void
}>()

const { legacyT } = useI18n()
const {
  isSyncing,
  syncNow,
  syncSummary,
  windowRows,
  statusBadge,
  formatDateTime,
  formatAmount,
} = useProviderRemoteQuota()

const remoteQuota = computed(() => props.provider.ops_remote_quota)
const sync = computed(() => remoteQuota.value?.sync ?? null)
const rows = computed(() => windowRows(sync.value))
const badge = computed(() => statusBadge(remoteQuota.value))
const syncing = computed(() => isSyncing(props.provider.id))
const actionFeedback = ref<{ ok: boolean; text: string } | null>(null)

const badgeVariant = computed(() => {
  switch (badge.value.tone) {
    case 'ok':
      return 'success'
    case 'warn':
      return 'warning'
    case 'error':
      return 'destructive'
    default:
      return 'secondary'
  }
})

const subscriptionText = computed(() => {
  const parts: string[] = []
  if (sync.value?.group_name) {
    parts.push(`${legacyT('分组')}: ${sync.value.group_name}`)
  }
  if (sync.value?.subscription_status) {
    parts.push(
      `${legacyT('订阅')}: ${sync.value.subscription_status}${
        sync.value.subscription_active ? '' : ` (${legacyT('无效')})`
      }`,
    )
  }
  return parts.join(' · ')
})

async function handleSyncNow() {
  actionFeedback.value = null
  try {
    const result = await syncNow(props.provider.id)
    const summary = syncSummary(result)
    if (result.status === 'success') {
      actionFeedback.value = {
        ok: true,
        text: result.message || legacyT('同步完成'),
      }
    } else {
      const detail = result.message || summary?.last_error || legacyT('同步失败')
      actionFeedback.value = { ok: false, text: detail }
    }
    // 成功与失败 payload 都会写回状态快照，通知父级刷新 provider 摘要
    emit('synced')
  } catch {
    actionFeedback.value = { ok: false, text: legacyT('网络错误，请稍后重试') }
  }
}
</script>
