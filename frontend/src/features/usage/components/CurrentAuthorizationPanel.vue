<template>
  <Card
    v-if="isOpen && authStore.isAdmin"
    data-testid="current-authorization-panel"
  >
    <div class="space-y-3 p-4">
      <div>
        <h4 class="font-medium">
          当前授权状态
        </h4>
        <p class="mt-1 text-xs text-muted-foreground">
          查询时的当前授权状态，不代表请求发生时的历史权限。账户与密钥状态通过后，不代表具体请求已通过额度、IP、模型等检查。
        </p>
      </div>

      <Button
        v-if="!snapshot && !loading && !error"
        type="button"
        size="sm"
        :disabled="!canQuery"
        data-testid="load-current-authorization"
        @click="load"
      >
        查看当前授权状态
      </Button>
      <p
        v-if="!canQuery"
        class="text-sm text-muted-foreground"
        data-testid="authorization-missing-ids"
      >
        该记录缺少调用方用户或 API Key 标识，无法查询当前授权状态。
      </p>
      <div
        v-if="loading"
        class="text-sm text-muted-foreground"
        data-testid="authorization-loading"
      >
        正在查询...
      </div>
      <div
        v-if="error"
        class="space-y-2"
        data-testid="authorization-error"
      >
        <p class="text-sm text-red-600 dark:text-red-400">
          {{ error }}
        </p>
        <Button
          type="button"
          size="sm"
          variant="outline"
          @click="load"
        >
          重试
        </Button>
      </div>

      <dl
        v-if="snapshot"
        class="grid grid-cols-1 gap-x-4 gap-y-2 text-sm sm:grid-cols-2"
        data-testid="authorization-snapshot"
      >
        <div>
          <dt class="text-muted-foreground">
            用户状态
          </dt><dd>{{ bool(snapshot.user_is_active, '活跃', '停用') }}{{ snapshot.user_is_deleted === true ? '（已删除）' : '' }}</dd>
        </div>
        <div>
          <dt class="text-muted-foreground">
            Key 状态
          </dt><dd>{{ bool(snapshot.api_key_is_active, '活跃', '停用') }}</dd>
        </div>
        <div>
          <dt class="text-muted-foreground">
            锁定
          </dt><dd>{{ bool(snapshot.api_key_is_locked, '是', '否') }}</dd>
        </div>
        <div>
          <dt class="text-muted-foreground">
            当前可用
          </dt><dd>{{ bool(snapshot.currently_usable, '是', '否') }}</dd>
        </div>
        <div>
          <dt class="text-muted-foreground">
            过期时间
          </dt><dd>{{ expiry(snapshot.api_key_expires_at_unix_secs) }}</dd>
        </div>
        <div>
          <dt class="text-muted-foreground">
            用户速率限制
          </dt><dd>{{ limit(snapshot.user_rate_limit, '请求/分钟') }}</dd>
        </div>
        <div>
          <dt class="text-muted-foreground">
            Key 速率限制
          </dt><dd>{{ limit(snapshot.api_key_rate_limit, '请求/分钟') }}</dd>
        </div>
        <div>
          <dt class="text-muted-foreground">
            Key 并发限制
          </dt><dd>{{ limit(snapshot.api_key_concurrent_limit, '并发') }}</dd>
        </div>
        <div>
          <dt class="text-muted-foreground">
            用户每日额度
          </dt><dd>{{ money(snapshot.user_daily_usage_limit_usd) }}</dd>
        </div>
        <div>
          <dt class="text-muted-foreground">
            Key 每日额度
          </dt><dd>{{ money(snapshot.api_key_daily_usage_limit_usd) }}</dd>
        </div>
        <div class="sm:col-span-2">
          <dt class="text-muted-foreground">
            用户允许的 Provider
          </dt><dd>{{ list(snapshot.user_allowed_providers) }}</dd>
        </div>
        <div class="sm:col-span-2">
          <dt class="text-muted-foreground">
            Key 允许的 Provider
          </dt><dd>{{ list(snapshot.api_key_allowed_providers) }}</dd>
        </div>
        <div class="sm:col-span-2">
          <dt class="text-muted-foreground">
            用户允许的 Model
          </dt><dd>{{ list(snapshot.user_allowed_models) }}</dd>
        </div>
        <div class="sm:col-span-2">
          <dt class="text-muted-foreground">
            Key 允许的 Model
          </dt><dd>{{ list(snapshot.api_key_allowed_models) }}</dd>
        </div>
        <div class="sm:col-span-2">
          <dt class="text-muted-foreground">
            用户允许的 API 格式
          </dt><dd>{{ list(snapshot.user_allowed_api_formats) }}</dd>
        </div>
        <div class="sm:col-span-2">
          <dt class="text-muted-foreground">
            Key 允许的 API 格式
          </dt><dd>{{ list(snapshot.api_key_allowed_api_formats) }}</dd>
        </div>
      </dl>
    </div>
  </Card>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useAuthStore } from '@/stores/auth'
import Card from '@/components/ui/card.vue'
import Button from '@/components/ui/button.vue'
import { getErrorStatus } from '@/types/api-error'
import { requestAuthApi, type ResolvedAuthApiKeySnapshot } from '@/api/requestAuth'

const props = defineProps<{ isOpen: boolean; requestId?: string | null; userId?: string | null; apiKeyId?: string | null }>()
const authStore = useAuthStore()
const snapshot = ref<ResolvedAuthApiKeySnapshot | null>(null)
const loading = ref(false)
const error = ref<string | null>(null)
let controller: AbortController | null = null
let requestSerial = 0

const canQuery = computed(() => Boolean(props.isOpen && authStore.isAdmin && props.userId && props.apiKeyId))

function cancel() {
  controller?.abort()
  controller = null
  requestSerial += 1
  loading.value = false
}

async function load() {
  if (!canQuery.value) return
  const userId = props.userId
  const apiKeyId = props.apiKeyId
  if (!userId || !apiKeyId) return
  cancel()
  const serial = requestSerial
  const requestController = new AbortController()
  controller = requestController
  loading.value = true
  error.value = null
  snapshot.value = null
  try {
    const result = await requestAuthApi.getCurrentAuthorization(userId, apiKeyId, requestController.signal)
    if (serial !== requestSerial || requestController.signal.aborted) return
    if (result.user_id !== userId || result.api_key_id !== apiKeyId) {
      error.value = '服务器返回的授权身份与查询目标不一致，无法显示结果。'
      return
    }
    snapshot.value = result
  } catch (err: unknown) {
    if (requestController.signal.aborted || serial !== requestSerial) return
    const status = getErrorStatus(err)
    error.value = status === 403 ? '你没有权限查看该授权状态。' : status === 404 ? '未找到该用户或 API Key。' : '查询授权状态失败，请检查网络后重试。'
  } finally {
    if (serial === requestSerial) {
      loading.value = false
      if (controller === requestController) controller = null
    }
  }
}

watch(() => [props.isOpen, props.requestId, props.userId, props.apiKeyId, authStore.isAdmin], () => {
  cancel()
  snapshot.value = null
  error.value = null
}, { flush: 'sync' })
onBeforeUnmount(cancel)

const bool = (value: boolean | undefined, yes: string, no: string) => value === undefined ? '未知' : value ? yes : no
const list = (values: string[] | null | undefined) => {
  return values === undefined || values === null ? '未提供' : values.length ? values.join(', ') : '无'
}
const limit = (value: number | null | undefined, unit: string) => value === undefined || value === null ? '未提供' : `${value} ${unit}`
const money = (value: number | null | undefined) => value === undefined || value === null ? '未提供' : `$${value}`
const expiry = (value: number | null | undefined) => value === undefined || value === null ? '未提供' : new Date(value * 1000).toLocaleString()
</script>
