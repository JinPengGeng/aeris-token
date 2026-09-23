<template>
  <div class="space-y-6 px-4 sm:px-6 lg:px-0">
    <div class="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-3">
      <div>
        <h1 class="text-lg font-semibold">
          毛利报表
        </h1>
        <p class="text-xs text-muted-foreground">
          收入(实收)− 供应商成本(落账快照),按 模型 × 供应商 × 时间粒度 透视
        </p>
      </div>
      <TimeRangePicker v-model="timeRange" />
    </div>

    <div class="flex flex-wrap items-center gap-2">
      <select
        v-model="granularity"
        class="h-8 rounded-md border border-input bg-background px-2 text-xs"
      >
        <option value="day">
          按天
        </option>
        <option value="week">
          按周
        </option>
        <option value="month">
          按月
        </option>
      </select>
      <span
        v-if="unknownRows > 0"
        class="text-xs text-amber-600 dark:text-amber-400"
      >
        {{ unknownRows }} 个组合存在未知成本(certainty=unknown),毛利标记为「未知」而非零成本
      </span>
    </div>

    <Card class="p-4">
      <div
        v-if="loading"
        class="py-8 text-center text-xs text-muted-foreground"
      >
        正在加载毛利数据...
      </div>
      <div
        v-else-if="rows.length === 0"
        class="py-8 text-center text-xs text-muted-foreground"
      >
        当前时间范围内暂无毛利数据
      </div>
      <div
        v-else
        class="overflow-x-auto"
      >
        <table class="w-full text-xs">
          <thead>
            <tr class="border-b text-left text-muted-foreground">
              <th class="py-2 pr-3 font-medium">
                周期
              </th>
              <th class="py-2 pr-3 font-medium">
                模型
              </th>
              <th class="py-2 pr-3 font-medium">
                供应商
              </th>
              <th class="py-2 pr-3 text-right font-medium">
                请求数
              </th>
              <th class="py-2 pr-3 text-right font-medium">
                收入
              </th>
              <th class="py-2 pr-3 text-right font-medium">
                成本
              </th>
              <th class="py-2 pr-3 text-right font-medium">
                毛利
              </th>
              <th class="py-2 pr-3 text-right font-medium">
                毛利率
              </th>
              <th class="py-2 pr-3 text-right font-medium">
                成本覆盖率
              </th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="row in rows"
              :key="`${row.period_start}|${row.model}|${row.provider_id}`"
              class="border-b last:border-0"
            >
              <td class="py-2 pr-3 whitespace-nowrap">
                {{ row.period_start }}
              </td>
              <td class="py-2 pr-3">
                {{ row.model }}
              </td>
              <td class="py-2 pr-3">
                {{ row.provider_id }}
              </td>
              <td class="py-2 pr-3 text-right">
                {{ row.request_count }}
              </td>
              <td class="py-2 pr-3 text-right whitespace-nowrap">
                {{ formatMoney(row.revenue, 8) }}
              </td>
              <td class="py-2 pr-3 text-right whitespace-nowrap">
                {{ formatMoney(row.cost, 8) }}
              </td>
              <td
                class="py-2 pr-3 text-right whitespace-nowrap"
                :class="row.margin === null ? 'text-amber-600 dark:text-amber-400' : marginClass(row.margin)"
              >
                {{ row.margin === null ? '未知' : formatMoney(row.margin, 8) }}
              </td>
              <td class="py-2 pr-3 text-right">
                {{ row.margin_rate === null ? '—' : `${row.margin_rate.toFixed(2)}%` }}
              </td>
              <td class="py-2 pr-3 text-right">
                {{ row.cost_coverage.estimated_share_percent.toFixed(2) }}%
                <span class="text-muted-foreground">
                  ({{ row.cost_coverage.unknown_requests > 0 ? `未知 ${row.cost_coverage.unknown_requests}` : '全量' }})
                </span>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </Card>
  </div>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import Card from '@/components/ui/card.vue'
import { TimeRangePicker } from '@/components/common'
import { usageApi, type UsageMarginGranularity, type UsageMarginRow } from '@/api/usage'
import { getDateRangeFromPeriod } from '@/features/usage/composables'
import { formatMoney, parseMoneyUnits } from '@/utils/money'
import type { DateRangeParams } from '@/features/usage/types'

const timeRange = ref<DateRangeParams>(getDateRangeFromPeriod('last30days'))
const granularity = ref<UsageMarginGranularity>('day')
const rows = ref<UsageMarginRow[]>([])
const loading = ref(false)

const unknownRows = computed(
  () => rows.value.filter((row) => row.cost_coverage.unknown_requests > 0).length
)

function marginClass(margin: string): string {
  const units = parseMoneyUnits(margin)
  if (units === null) return ''
  return units >= 0 ? 'text-green-600 dark:text-green-400' : 'text-red-600 dark:text-red-400'
}

async function loadReport() {
  loading.value = true
  try {
    rows.value = await usageApi.getUsageMarginStats(
      {
        ...timeRange.value,
        granularity: granularity.value,
        limit: 500
      },
      { skipCache: true }
    )
  } finally {
    loading.value = false
  }
}

watch([timeRange, granularity], loadReport, { deep: true, immediate: true })
</script>
