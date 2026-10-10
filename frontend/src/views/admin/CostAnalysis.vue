<template>
  <main class="space-y-6 px-4 pb-8 sm:px-6 lg:px-0">
    <OverviewToolbar
      :title="t('成本分析', 'Cost analysis')"
      :range="range"
      :preset="relativePreset"
      :refresh-active="autoRefresh"
      :refresh-title="refreshTitle"
      :show-range="false"
      presets-only
      :loading="loading"
      @update:range="setRange"
      @refresh="toggleAutoRefresh"
    >
      <Button
        variant="outline"
        size="sm"
        :disabled="!accounts?.items.length"
        @click="recording = true"
      >
        <Plus class="mr-1.5 h-3.5 w-3.5" />{{ t('登记支出', 'Record expense') }}
      </Button>
    </OverviewToolbar>
    <div
      class="grid grid-cols-2 gap-3 xl:grid-cols-4"
      data-cost-summary
    >
      <Card class="min-w-0 space-y-2 p-4 sm:p-5">
        <p class="text-xs text-muted-foreground">
          {{ t('已登记支出', 'Recorded expenses') }}
        </p>
        <div class="min-h-8 text-xl font-semibold tabular-nums">
          <p
            v-for="item in totalAmounts('amount')"
            :key="item.currency"
          >
            {{ item.text }}
          </p>
        </div>
        <p class="text-xs text-muted-foreground">
          {{ t('所选时段', 'Selected range') }} · {{ count(expenses?.total) }} {{ t('笔', 'entries') }}
        </p>
      </Card>
      <Card class="min-w-0 space-y-2 p-4 sm:p-5">
        <p class="text-xs text-muted-foreground">
          {{ t('提供商充值', 'Provider recharges') }}
        </p>
        <div class="min-h-8 text-xl font-semibold tabular-nums">
          <p
            v-for="item in totalAmounts('recharge_amount')"
            :key="item.currency"
          >
            {{ item.text }}
          </p>
        </div>
        <p class="text-xs text-muted-foreground">
          {{ t('已登记的余额充值', 'Recorded balance top-ups') }}
        </p>
      </Card>
      <Card class="min-w-0 space-y-2 p-4 sm:p-5">
        <p class="text-xs text-muted-foreground">
          {{ t('套餐与续费', 'Subscriptions and renewals') }}
        </p>
        <div class="min-h-8 text-xl font-semibold tabular-nums">
          <p
            v-for="item in totalAmounts('subscription_amount')"
            :key="item.currency"
          >
            {{ item.text }}
          </p>
        </div>
        <p class="text-xs text-muted-foreground">
          {{ t('已登记的套餐实付金额', 'Recorded subscription payments') }}
        </p>
      </Card>
      <Card class="min-w-0 space-y-2 p-4 sm:p-5">
        <p class="text-xs text-muted-foreground">
          {{ t('提供商调用', 'Provider requests') }}
        </p>
        <p class="min-h-8 text-xl font-semibold tabular-nums">
          {{ count(costs?.data.summary.request_count) }}
        </p>
        <p class="text-xs text-muted-foreground">
          {{ count(costs?.data.summary.total_tokens) }} Tokens
        </p>
      </Card>
    </div>
    <p class="text-xs text-muted-foreground">
      {{ t('支出以登记的付款记录为准，不同币种分别汇总；套餐额度与上游余额单独展示。', 'Expenses reflect recorded payments, grouped by currency. Plan quotas and upstream balances are shown separately.') }}
    </p>

    <div class="grid grid-cols-1 xl:grid-cols-2 gap-4">
      <UsageProviderTable
        :data="providerStats"
        :is-admin="true"
      />

      <Card class="p-4 space-y-4">
        <div class="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
          <div>
            <h3 class="text-sm font-medium">
              提供商成本归因
            </h3>
            <p class="text-xs text-muted-foreground">
              查看选定提供商在当前时间范围内的用户贡献占比
            </p>
          </div>
          <div class="flex flex-wrap gap-2">
            <select
              v-model="selectedProviderKey"
              class="h-8 rounded-md border border-input bg-background px-2 text-xs"
            >
              <option
                v-for="provider in providerStats"
                :key="provider.providerKey ?? provider.providerId ?? provider.provider"
                :value="provider.providerKey ?? provider.providerId ?? provider.provider"
              >
                {{ provider.provider }}
              </option>
            </select>
            <select
              v-model="attributionMetric"
              class="h-8 rounded-md border border-input bg-background px-2 text-xs"
            >
              <option value="actual_cost">
                实际成本
              </option>
              <option value="total_cost">
                展示成本
              </option>
              <option value="tokens">
                Tokens
              </option>
              <option value="requests">
                请求数
              </option>
            </select>
          </div>
        </div>

        <div
          v-if="providerAttributionLoading"
          class="py-8 text-center text-xs text-muted-foreground"
        >
          正在加载归因数据...
        </div>
        <div
          v-else-if="!selectedProvider"
          class="py-8 text-center text-xs text-muted-foreground"
        >
          暂无可归因的提供商数据
        </div>
        <div
          v-else-if="providerAttributionVisibleItems.length === 0"
          class="py-8 text-center text-xs text-muted-foreground"
        >
          当前时间范围内暂无用户贡献数据
        </div>
        <div
          v-else
          class="space-y-3"
        >
          <div class="rounded-lg border p-3">
            <div class="flex items-center justify-between text-xs text-muted-foreground">
              <span>{{ selectedProvider.provider }}</span>
              <span>{{ attributionMetricLabel }}</span>
            </div>
            <div class="mt-1 text-lg font-semibold">
              {{ formatAttributionMetric(providerAttribution?.total ?? 0) }}
            </div>
          </div>

          <div
            v-if="showAttributionDonut"
            class="grid gap-4 lg:grid-cols-[minmax(0,220px)_1fr] lg:items-center"
          >
            <div class="h-[220px] min-w-0">
              <DoughnutChart
                :data="providerAttributionDonutData"
                :options="providerAttributionDonutOptions"
              />
            </div>
            <div class="space-y-2">
              <div
                v-for="(item, index) in providerAttributionVisibleItems"
                :key="item.id"
                class="flex items-center justify-between gap-3 rounded-md border bg-muted/20 px-3 py-2 text-xs"
              >
                <div class="flex min-w-0 items-center gap-2">
                  <span
                    class="h-2.5 w-2.5 shrink-0 rounded-full"
                    :style="{ backgroundColor: ATTRIBUTION_DONUT_COLORS[index % ATTRIBUTION_DONUT_COLORS.length] }"
                  />
                  <span class="truncate font-medium">{{ item.name }}</span>
                </div>
                <div class="shrink-0 text-right">
                  <div class="font-medium">
                    {{ formatAttributionMetricValue(item) }}
                  </div>
                  <div class="text-[11px] text-muted-foreground">
                    {{ formatShare(item.share) }}
                  </div>
                </div>
              </div>
            </div>
          </div>

          <div
            v-else
            class="space-y-2"
          >
            <div
              v-for="item in providerAttributionVisibleItems"
              :key="item.id"
              class="space-y-1"
            >
              <div class="flex items-center justify-between gap-3 text-xs">
                <span class="truncate font-medium">{{ item.name }}</span>
                <span class="text-muted-foreground">{{ formatShare(item.share) }}</span>
              </div>
              <div class="h-2 overflow-hidden rounded-full bg-muted">
                <div
                  class="h-full rounded-full bg-primary"
                  :style="{ width: `${Math.min(item.share * 100, 100)}%` }"
                />
              </div>
              <div class="flex flex-wrap gap-x-3 gap-y-1 text-[11px] text-muted-foreground">
                <span>请求 {{ item.requests }}</span>
                <span>Tokens {{ formatTokens(item.total_tokens) }}</span>
                <span>展示 {{ formatCurrency(item.total_cost) }}</span>
                <span>实际 {{ formatCurrency(item.actual_cost) }}</span>
              </div>
            </div>
          </div>
        </div>
      </Card>
    </div>

    <section
      class="space-y-4"
      data-provider-usage
    >
      <div class="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h2 class="text-sm font-semibold">
            {{ t('提供商用量', 'Provider usage') }}
          </h2><p class="mt-1 text-xs text-muted-foreground">
            {{ t('所选时段 · 按请求量排序', 'Selected range · Most requests first') }}
          </p>
        </div>
        <Button
          variant="outline"
          size="sm"
          :disabled="exporting || !providers?.data.total"
          @click="exportCsv('breakdown', providerQuery)"
        >
          <Download class="mr-1.5 h-3.5 w-3.5" />{{ t('导出用量报表', 'Export usage') }}
        </Button>
      </div>
      <OverviewStatus
        :loading="providerLoading && !providers"
        :error="providerError || exportError"
        @retry="refreshProviders"
      />
      <Card class="min-w-0 overflow-hidden">
        <div class="overflow-x-auto px-4 sm:px-5">
          <table class="w-full min-w-[760px] text-sm">
            <thead class="border-b text-xs text-muted-foreground">
              <tr>
                <th class="py-3 text-left font-medium">
                  {{ t('提供商', 'Provider') }}
                </th>
                <th class="px-3 py-3 text-right font-medium">
                  {{ t('请求量', 'Requests') }}
                </th>
                <th class="px-3 py-3 text-right font-medium">
                  {{ t('输入 Token', 'Input tokens') }}
                </th>
                <th class="px-3 py-3 text-right font-medium">
                  {{ t('输出 Token', 'Output tokens') }}
                </th>
                <th class="px-3 py-3 text-right font-medium">
                  {{ t('已登记支出', 'Recorded expenses') }}
                </th>
                <th class="py-3 text-right font-medium">
                  {{ t('使用记录', 'Usage records') }}
                </th>
              </tr>
            </thead>
            <tbody class="divide-y">
              <tr
                v-for="provider in providers?.data.items || []"
                :key="provider.id || 'unattributed'"
              >
                <td class="max-w-64 break-words py-4 font-medium">
                  {{ provider.label || t('未归属提供商', 'Unattributed provider') }}<p class="mt-1 text-xs font-normal text-muted-foreground">
                    {{ providerType(provider.id) }}
                  </p>
                </td>
                <td class="px-3 py-4 text-right tabular-nums">
                  {{ count(provider.request_count) }}
                </td>
                <td class="px-3 py-4 text-right tabular-nums">
                  {{ count(provider.input_tokens) }}
                </td>
                <td class="px-3 py-4 text-right tabular-nums">
                  {{ count(provider.output_tokens) }}
                </td>
                <td class="px-3 py-4 text-right tabular-nums">
                  <p
                    v-for="item in providerAmounts(provider.id)"
                    :key="item.currency"
                    class="whitespace-nowrap"
                  >
                    {{ item.text }}
                  </p>
                </td>
                <td class="py-4 text-right">
                  <RouterLink
                    v-if="provider.id"
                    :to="link('/admin/usage', { provider_id: provider.id })"
                    class="inline-flex items-center gap-1 text-xs text-primary hover:underline"
                  >
                    {{ t('查看', 'View') }}<ArrowUpRight class="h-3.5 w-3.5" />
                  </RouterLink><span v-else>-</span>
                </td>
              </tr>
              <tr v-if="!providers?.data.items.length">
                <td
                  colspan="6"
                  class="py-12 text-center text-muted-foreground"
                >
                  {{ providerLoading ? t('加载提供商用量…', 'Loading provider usage…') : providers ? t('此时段暂无调用记录', 'No requests in this range') : t('暂未取得用量', 'Usage unavailable') }}
                </td>
              </tr>
            </tbody>
          </table>
        </div>
        <div
          v-if="providers"
          class="px-4 pb-3 sm:px-5"
        >
          <OverviewPagination
            :total="providers.data.total"
            :limit="providerLimit"
            :offset="providerOffset"
            :loading="providerLoading"
            @page="value => providerOffset = value"
            @size="value => { providerLimit = value; providerOffset = 0 }"
          />
        </div>
      </Card>
    </section>

    <OverviewStatus
      :loading="accountLoading && !accounts"
      :error="accountError"
      @retry="refreshAccounts"
    />
    <ProviderAccounts
      v-if="accounts"
      :accounts="accounts.items"
      :observed-at="accounts.observed_at"
      :timezone="range.timezone"
    />

    <OverviewStatus
      :loading="expenseLoading && !expenses"
      :error="expenseError"
      @retry="refreshExpenses"
    />
    <ExpenseLedger
      :data="expenses"
      :range="range"
      :offset="expenseOffset"
      :loading="expenseLoading"
      :can-record="Boolean(accounts?.items.length)"
      @record="recording = true"
      @page="selectExpensePage"
      @size="value => { expenseLimit = value; expenseOffset = 0 }"
      @changed="expensesChanged"
    />

    <section
      class="space-y-4"
      data-cost-reports
    >
      <h2 class="text-sm font-semibold">
        {{ t('用量报表', 'Usage reports') }}
      </h2>
      <OverviewStatus
        :loading="costLoading && !costs"
        :error="costError"
        @retry="refreshCosts"
      />
      <div class="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <Card class="min-w-0 space-y-4 p-4 sm:p-5">
          <h3 class="text-sm font-medium">
            {{ t('调用趋势', 'Request trend') }}
          </h3><div
            v-if="hasRequests"
            class="h-60"
          >
            <LineChart :data="requestChart" />
          </div><p
            v-else
            class="flex h-60 items-center justify-center text-sm text-muted-foreground"
          >
            {{ costs ? t('此时段暂无调用', 'No requests in this range') : t('暂无报表数据', 'Report unavailable') }}
          </p>
        </Card>
        <Card class="min-w-0 space-y-4 p-4 sm:p-5">
          <h3 class="text-sm font-medium">
            {{ t('Token 用量趋势', 'Token usage trend') }}
          </h3><div
            v-if="hasRequests"
            class="h-60"
          >
            <LineChart :data="tokenChart" />
          </div><p
            v-else
            class="flex h-60 items-center justify-center text-sm text-muted-foreground"
          >
            {{ costs ? t('此时段暂无用量', 'No usage in this range') : t('暂无报表数据', 'Report unavailable') }}
          </p>
        </Card>
      </div>
    </section>
    <ExpenseDialog
      v-model:open="recording"
      :providers="accounts?.items || []"
      :timezone="range.timezone"
      @saved="expensesChanged"
    />
  </main>
</template>


<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from 'vue'
import type { ChartData, ChartOptions } from 'chart.js'
import { RouterLink } from 'vue-router'
import { ArrowUpRight, Download, Plus } from 'lucide-vue-next'
import { Button, Card } from '@/components/ui'
import LineChart from '@/components/charts/LineChart.vue'
import DoughnutChart from '@/components/charts/DoughnutChart.vue'
import { UsageProviderTable } from '@/features/usage/components'
import { adminApi, type CostForecastResponse, type CostSavingsResponse, type LeaderboardItem, type QuotaUsageProvider } from '@/api/admin'
import { usageApi, type UsageAttributionItem, type UsageAttributionMetric, type UsageAttributionResponse } from '@/api/usage'
import { overviewApi } from '@/api/overview'
import { providerFinanceApi, type ProviderExpenseTotals } from '@/api/providerFinance'
import OverviewToolbar from '@/features/overview/components/OverviewToolbar.vue'
import OverviewStatus from '@/features/overview/components/OverviewStatus.vue'
import OverviewPagination from '@/features/overview/components/OverviewPagination.vue'
import ExpenseDialog from '@/features/overview/costs/ExpenseDialog.vue'
import ExpenseLedger from '@/features/overview/costs/ExpenseLedger.vue'
import ProviderAccounts from '@/features/overview/costs/ProviderAccounts.vue'
import { useOverviewQuery } from '@/features/overview/query'
import { useOverviewAutoRefresh } from '@/features/overview/useOverviewAutoRefresh'
import { useOverviewRequest } from '@/features/overview/useOverviewRequest'
import { useOverviewExport } from '@/features/overview/useOverviewExport'
import { useOverviewI18n } from '@/features/overview/i18n'
import { count, money, timestamp } from '@/features/overview/format'
import { formatCurrency, formatTokens } from '@/utils/format'
import { getDateRangeFromPeriod } from '@/features/usage/composables'
import { normalizeUsageProviderStats } from '@/features/usage/utils/providerStats'
import type { DateRangeParams } from '@/features/usage/types'
import type { ProviderStatsItem } from '@/features/usage/types'

const { t } = useOverviewI18n()
const { range, relativePreset, refreshRange, setRange, link } = useOverviewQuery('last30days', { rolling: true })
const { revision, autoRefresh, refreshTitle, toggleAutoRefresh } = useOverviewAutoRefresh(refreshRange)
const recording = ref(false)
const providerOffset = ref(0)
const expenseOffset = ref(0)
const providerLimit = ref(25)
const expenseLimit = ref(25)
const selectedScope = computed(() => JSON.stringify(relativePreset.value ? { preset: relativePreset.value, timezone: range.value.timezone } : range.value))
watch(selectedScope, () => { providerOffset.value = 0; expenseOffset.value = 0 })
const costQuery = computed(() => ({ ...range.value, granularity: 'day' as const }))
const providerQuery = computed(() => ({ ...range.value, group_by: 'provider', sort: 'request_count', order: 'desc' as const, limit: providerLimit.value, offset: providerOffset.value }))
const expenseQuery = computed(() => ({ ...range.value, limit: expenseLimit.value, offset: expenseOffset.value }))
const { data: costs, loading: costLoading, error: costError, refresh: refreshCosts } = useOverviewRequest(() => JSON.stringify([costQuery.value, revision.value]), signal => overviewApi.costs(costQuery.value, signal), { scopeKey: selectedScope })
const { data: providers, loading: providerLoading, error: providerError, refresh: refreshProviders } = useOverviewRequest(() => JSON.stringify([providerQuery.value, revision.value]), signal => overviewApi.breakdown(providerQuery.value, signal), { scopeKey: () => JSON.stringify([selectedScope.value, providerOffset.value]) })
const { data: accounts, loading: accountLoading, error: accountError, refresh: refreshAccounts } = useOverviewRequest(revision, signal => providerFinanceApi.accounts(signal))
const { data: expenses, loading: expenseLoading, error: expenseError, refresh: refreshExpenses } = useOverviewRequest(() => JSON.stringify([expenseQuery.value, revision.value]), signal => providerFinanceApi.expenses(expenseQuery.value, signal), { scopeKey: selectedScope })
const { exporting, exportError, exportCsv } = useOverviewExport()
const loading = computed(() => costLoading.value || providerLoading.value || accountLoading.value || expenseLoading.value)
function expensesChanged() { if (expenseOffset.value) expenseOffset.value = 0; else void refreshExpenses() }
function selectExpensePage(offset: number) { if (expenseOffset.value === offset) void refreshExpenses(); else expenseOffset.value = offset }
function amount(value: string, currency: string) { return money({ value, currency, basis: 'provider_expense', status: 'known' }) }
function totalAmounts(key: keyof Pick<ProviderExpenseTotals, 'amount' | 'recharge_amount' | 'subscription_amount'>) {
  if (!expenses.value) return [{ currency: '', text: '-' }]
  if (!expenses.value.totals.length) return [{ currency: '', text: t('暂无记录', 'No records') }]
  return expenses.value.totals.map(row => ({ currency: row.currency, text: `${amount(row[key], row.currency)} ${row.currency}` }))
}
function providerAmounts(id: string | null) {
  if (!expenses.value || !id) return [{ currency: '', text: '-' }]
  const amounts = expenses.value.providers.filter(item => item.provider_id === id)
  return amounts.length ? amounts.map(item => ({ currency: item.currency, text: `${amount(item.amount, item.currency)} ${item.currency}` })) : [{ currency: '', text: t('未登记', 'Unrecorded') }]
}
function providerType(id: string | null) {
  const type = accounts.value?.items.find(account => account.provider_id === id)?.billing_type
  return type === 'monthly_quota' ? t('周期套餐', 'Period plan') : type === 'free_tier' ? t('免费额度', 'Free tier') : type === 'pay_as_you_go' ? t('按量付费', 'Pay as you go') : ''
}

const timeRange = ref<DateRangeParams>(getDateRangeFromPeriod('last30days'))

const forecast = ref<CostForecastResponse | null>(null)
const costSavings = ref<CostSavingsResponse | null>(null)
const quotaProviders = ref<QuotaUsageProvider[]>([])
const providerStats = ref<ProviderStatsItem[]>([])
const selectedProviderKey = ref('')
const providerAttribution = ref<UsageAttributionResponse | null>(null)
const attributionMetric = ref<UsageAttributionMetric>('actual_cost')
const apiKeyLeaderboard = ref<LeaderboardItem[]>([])
const apiKeyLeaderboardMetric = ref<'requests' | 'tokens' | 'cost'>('cost')
const apiKeyLeaderboardTimeRange = ref<DateRangeParams>(getDateRangeFromPeriod('last30days'))
const apiKeyLeaderboardPage = ref(1)
const apiKeyLeaderboardPageSize = ref(10)
const apiKeyLeaderboardTotal = ref(0)
const ATTRIBUTION_DONUT_THRESHOLD = 8
const ATTRIBUTION_DONUT_COLORS = [
  'rgba(59, 130, 246, 0.82)',
  'rgba(239, 68, 68, 0.82)',
  'rgba(16, 185, 129, 0.82)',
  'rgba(245, 158, 11, 0.82)',
  'rgba(139, 92, 246, 0.82)',
  'rgba(6, 182, 212, 0.82)',
  'rgba(132, 204, 22, 0.82)',
  'rgba(249, 115, 22, 0.82)',
  'rgba(148, 163, 184, 0.82)'
]

const forecastLoading = ref(false)
const quotaLoading = ref(false)
const providerAttributionLoading = ref(false)
const apiKeyLeaderboardLoading = ref(false)
let forecastRequestId = 0
let savingsRequestId = 0
let quotaRequestId = 0
let providerStatsRequestId = 0
let providerAttributionRequestId = 0
let apiKeyLeaderboardRequestId = 0
let loadAllPromise: Promise<void> | null = null
let hasPendingLoadAll = false
let loadAllDebounceTimer: ReturnType<typeof setTimeout> | null = null
let providerAttributionDebounceTimer: ReturnType<typeof setTimeout> | null = null
let apiKeyLeaderboardDebounceTimer: ReturnType<typeof setTimeout> | null = null

const selectedProvider = computed(() => providerStats.value.find(provider => providerSelectionKey(provider) === selectedProviderKey.value))
const providerAttributionItems = computed(() => providerAttribution.value?.items ?? [])
const providerAttributionDisplayItems = computed<UsageAttributionItem[]>(() => {
  const items = [...providerAttributionItems.value]
  const others = providerAttribution.value?.others
  if (others && others.requests > 0) {
    items.push({ ...others, id: 'others', name: others.name || 'Others' })
  }
  return items
})
const providerAttributionVisibleItems = computed<UsageAttributionItem[]>(() =>
  providerAttributionDisplayItems.value.filter(item => attributionMetricValue(item) > 0)
)
const showAttributionDonut = computed(() => {
  const items = providerAttributionVisibleItems.value
  return items.length > 0 && items.length < ATTRIBUTION_DONUT_THRESHOLD
})
const providerAttributionDonutData = computed<ChartData<'doughnut'>>(() => ({
  labels: providerAttributionVisibleItems.value.map(item => item.name),
  datasets: [{
    data: providerAttributionVisibleItems.value.map(item => attributionMetricValue(item)),
    backgroundColor: providerAttributionVisibleItems.value.map((_, index) => ATTRIBUTION_DONUT_COLORS[index % ATTRIBUTION_DONUT_COLORS.length]),
    borderWidth: 2,
    borderColor: 'rgba(255, 255, 255, 0.12)'
  }]
}))
const providerAttributionDonutOptions = computed<ChartOptions<'doughnut'>>(() => ({
  responsive: true,
  maintainAspectRatio: false,
  cutout: '64%',
  plugins: {
    legend: {
      display: false
    },
    tooltip: {
      callbacks: {
        label: (context) => {
          const value = typeof context.raw === 'number' ? context.raw : 0
          const total = (context.dataset.data as number[]).reduce((sum, current) => sum + current, 0)
          const percentage = total > 0 ? ((value / total) * 100).toFixed(1) : '0.0'
          return `${context.label}: ${formatAttributionMetric(value)} (${percentage}%)`
        }
      }
    }
  }
}))
const attributionMetricLabel = computed(() => {
  switch (attributionMetric.value) {
    case 'actual_cost': return '实际成本'
    case 'total_cost': return '展示成本'
    case 'tokens': return 'Tokens'
    case 'requests': return '请求数'
    default: return '指标'
  }
})

function buildTimeRangeParams() {
  return {
    start_date: timeRange.value.start_date,
    end_date: timeRange.value.end_date,
    preset: timeRange.value.preset,
    timezone: timeRange.value.timezone,
    tz_offset_minutes: timeRange.value.tz_offset_minutes
  }
}

function providerSelectionKey(provider: ProviderStatsItem) {
  return provider.providerKey ?? provider.providerId ?? provider.provider
}

function formatShare(share: number) {
  return `${(share * 100).toFixed(1)}%`
}

function formatAttributionMetric(value: number) {
  if (attributionMetric.value === 'actual_cost' || attributionMetric.value === 'total_cost') {
    return formatCurrency(value)
  }
  if (attributionMetric.value === 'tokens') {
    return formatTokens(value)
  }
  return Math.round(value).toLocaleString('zh-CN')
}

function attributionMetricValue(item: UsageAttributionItem) {
  switch (attributionMetric.value) {
    case 'actual_cost': return item.actual_cost
    case 'total_cost': return item.total_cost
    case 'tokens': return item.total_tokens
    case 'requests': return item.requests
    default: return 0
  }
}

function formatAttributionMetricValue(item: UsageAttributionItem) {
  return formatAttributionMetric(attributionMetricValue(item))
}

async function loadForecast() {
  const requestId = ++forecastRequestId
  forecastLoading.value = true
  try {
    const data = await adminApi.getCostForecast(buildTimeRangeParams())
    if (requestId !== forecastRequestId) return
    forecast.value = data
  } finally {
    if (requestId === forecastRequestId) {
      forecastLoading.value = false
    }

  }
}
async function loadSavings() {
  const requestId = ++savingsRequestId
  const data = await adminApi.getCostSavings(buildTimeRangeParams())
  if (requestId !== savingsRequestId) return
  costSavings.value = data
}

async function loadQuotaUsage() {
  const requestId = ++quotaRequestId
  quotaLoading.value = true
  try {
    const response = await adminApi.getQuotaUsage()
    if (requestId !== quotaRequestId) return
    quotaProviders.value = response.providers
  } finally {
    if (requestId === quotaRequestId) {
      quotaLoading.value = false
    }
  }
}

async function loadProviderStats() {
  const requestId = ++providerStatsRequestId
  const stats = await usageApi.getUsageByProvider({
    ...buildTimeRangeParams(),
    limit: 8
  })
  if (requestId !== providerStatsRequestId) return
  providerStats.value = normalizeUsageProviderStats(stats)
  if (!providerStats.value.some(provider => providerSelectionKey(provider) === selectedProviderKey.value)) {
    selectedProviderKey.value = providerStats.value[0] ? providerSelectionKey(providerStats.value[0]) : ''
  }
  scheduleProviderAttributionLoad()
}

async function loadProviderAttribution() {
  const provider = selectedProvider.value
  if (!provider) {
    providerAttribution.value = null
    return
  }
  const requestId = ++providerAttributionRequestId
  providerAttributionLoading.value = true
  try {
    const response = await usageApi.getUsageAttribution({
      ...buildTimeRangeParams(),
      provider_id: provider.providerId && provider.providerIdentitySource !== 'legacy_name' ? provider.providerId : undefined,
      provider_name: provider.providerIdentitySource === 'legacy_name' || !provider.providerId ? provider.provider : undefined,
      group_by: 'user',
      metric: attributionMetric.value,
      limit: ATTRIBUTION_DONUT_THRESHOLD,
    })
    if (requestId !== providerAttributionRequestId) return
    providerAttribution.value = response
  } finally {
    if (requestId === providerAttributionRequestId) {
      providerAttributionLoading.value = false
    }
  }
}

async function loadApiKeyLeaderboard() {
  const requestId = ++apiKeyLeaderboardRequestId
  apiKeyLeaderboardLoading.value = true
  try {
    const response = await adminApi.getLeaderboardApiKeys({
      ...buildApiKeyLeaderboardTimeRangeParams(),
      metric: apiKeyLeaderboardMetric.value,
      order: 'desc',
      limit: apiKeyLeaderboardPageSize.value,
      offset: (apiKeyLeaderboardPage.value - 1) * apiKeyLeaderboardPageSize.value,
      include_inactive: false,
      exclude_admin: false
    })
    if (requestId !== apiKeyLeaderboardRequestId) return
    apiKeyLeaderboard.value = response.items
    apiKeyLeaderboardTotal.value = response.total
    if (response.items.length === 0 && response.total > 0 && apiKeyLeaderboardPage.value > 1) {
      apiKeyLeaderboardPage.value = 1
      scheduleApiKeyLeaderboardLoad()
    }
  } finally {
    if (requestId === apiKeyLeaderboardRequestId) {
      apiKeyLeaderboardLoading.value = false
    }
  }
}

function buildApiKeyLeaderboardTimeRangeParams() {
  return {
    start_date: apiKeyLeaderboardTimeRange.value.start_date,
    end_date: apiKeyLeaderboardTimeRange.value.end_date,
    preset: apiKeyLeaderboardTimeRange.value.preset,
    timezone: apiKeyLeaderboardTimeRange.value.timezone,
    tz_offset_minutes: apiKeyLeaderboardTimeRange.value.tz_offset_minutes
  }
}

async function loadAll() {
  if (loadAllPromise) {
    hasPendingLoadAll = true
    return loadAllPromise
  }
  loadAllPromise = Promise.all([
    loadForecast(),
    loadSavings(),
    loadQuotaUsage(),
    loadProviderStats(),
    loadProviderAttribution(),
    loadApiKeyLeaderboard()
  ])
    .then(() => undefined)
    .finally(() => {
      loadAllPromise = null
      if (hasPendingLoadAll) {
        hasPendingLoadAll = false
        void loadAll()
      }
    })
  return loadAllPromise
}

function scheduleLoadAll() {
  if (loadAllDebounceTimer) {
    clearTimeout(loadAllDebounceTimer)
  }
  loadAllDebounceTimer = setTimeout(() => {
    loadAllDebounceTimer = null
    void loadAll()
  }, 120)
}

function scheduleApiKeyLeaderboardLoad() {
  if (apiKeyLeaderboardDebounceTimer) {
    clearTimeout(apiKeyLeaderboardDebounceTimer)
  }
  apiKeyLeaderboardDebounceTimer = setTimeout(() => {
    apiKeyLeaderboardDebounceTimer = null
    void loadApiKeyLeaderboard()
  }, 120)
}

function scheduleProviderAttributionLoad() {
  if (providerAttributionDebounceTimer) {
    clearTimeout(providerAttributionDebounceTimer)
  }
  providerAttributionDebounceTimer = setTimeout(() => {
    providerAttributionDebounceTimer = null
    void loadProviderAttribution()
  }, 120)
}

function resetApiKeyLeaderboardPage() {
  if (apiKeyLeaderboardPage.value === 1) {
    return
  }
  apiKeyLeaderboardPage.value = 1
}

watch(timeRange, () => {
  resetApiKeyLeaderboardPage()
  scheduleLoadAll()
  scheduleProviderAttributionLoad()
}, { deep: true })
watch([selectedProviderKey, attributionMetric], scheduleProviderAttributionLoad)
watch(apiKeyLeaderboardMetric, () => {
  resetApiKeyLeaderboardPage()
  scheduleApiKeyLeaderboardLoad()
})
watch(apiKeyLeaderboardTimeRange, () => {
  resetApiKeyLeaderboardPage()
  scheduleApiKeyLeaderboardLoad()
}, { deep: true })
watch([apiKeyLeaderboardPage, apiKeyLeaderboardPageSize], scheduleApiKeyLeaderboardLoad)

onMounted(() => {
  void loadAll()
})

onUnmounted(() => {
  if (loadAllDebounceTimer) {
    clearTimeout(loadAllDebounceTimer)
    loadAllDebounceTimer = null
  }
  if (apiKeyLeaderboardDebounceTimer) {
    clearTimeout(apiKeyLeaderboardDebounceTimer)
    apiKeyLeaderboardDebounceTimer = null
  }
  if (providerAttributionDebounceTimer) {
    clearTimeout(providerAttributionDebounceTimer)
    providerAttributionDebounceTimer = null
  }
  hasPendingLoadAll = false
  loadAllPromise = null
  forecastRequestId += 1
  savingsRequestId += 1
  quotaRequestId += 1
  providerStatsRequestId += 1
  providerAttributionRequestId += 1
  apiKeyLeaderboardRequestId += 1
})
const points = computed(() => costs.value?.data.timeseries || [])
const hasRequests = computed(() => points.value.some(point => point.request_count > 0))
const labels = computed(() => points.value.map(point => timestamp(point.bucket_start, range.value.timezone)))
const requestChart = computed(() => ({ labels: labels.value, datasets: [{ label: t('请求数', 'Requests'), data: points.value.map(point => point.request_count), borderColor: '#ce6545', backgroundColor: '#ce654518', fill: true, pointRadius: 1, borderWidth: 2, tension: 0.2 }] }))
const tokenChart = computed(() => ({ labels: labels.value, datasets: [{ label: t('输入 Token', 'Input tokens'), data: points.value.map(point => point.input_tokens), borderColor: '#0d9488', pointRadius: 1, borderWidth: 2, tension: 0.2 }, { label: t('输出 Token', 'Output tokens'), data: points.value.map(point => point.output_tokens), borderColor: '#d97706', pointRadius: 1, borderWidth: 2, tension: 0.2 }] }))
</script>
