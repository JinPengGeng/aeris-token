<template>
  <Dialog
    :model-value="open"
    title="成本价目"
    description="维护 Provider 该模型的成本价目，catalog 形状与售价 catalog 一致，供营收保护计费使用"
    :icon="Coins"
    size="xl"
    @update:model-value="handleClose"
  >
    <div class="space-y-4">
      <div class="rounded-lg border bg-muted/30 p-3">
        <p class="text-sm text-muted-foreground font-mono">
          {{ providerId }} / {{ modelName }}
        </p>
      </div>

      <!-- 现有价目 -->
      <div class="space-y-2">
        <div class="flex items-center justify-between">
          <Label class="text-sm font-medium">价目列表</Label>
          <Button
            type="button"
            variant="outline"
            size="sm"
            @click="startCreate"
          >
            <Plus class="w-4 h-4 mr-1" />
            新增价目
          </Button>
        </div>
        <p
          v-if="loadError"
          class="text-sm text-destructive"
        >
          {{ loadError }}
        </p>
        <p
          v-else-if="!loading && records.length === 0"
          class="text-sm text-muted-foreground"
        >
          暂无成本价目
        </p>
        <div
          v-for="record in records"
          :key="record.cost_id"
          class="flex items-center justify-between gap-3 px-3 py-2 rounded-lg border border-border/50"
        >
          <div class="min-w-0">
            <p class="text-sm font-medium">
              {{ taskTypeLabel(record.task_type) }}
              <span class="ml-2 text-xs text-muted-foreground">{{ record.currency }}</span>
            </p>
            <p class="text-xs text-muted-foreground font-mono truncate">
              {{ formatWindow(record) }}
              <template v-if="record.price_per_request != null">
                · 按次 {{ record.price_per_request }}
              </template>
              <template v-if="record.tiered_pricing">
                · 分档 catalog
              </template>
            </p>
          </div>
          <div class="flex items-center gap-1 shrink-0">
            <Button
              type="button"
              variant="ghost"
              size="icon"
              class="h-8 w-8"
              title="编辑"
              @click="startEdit(record)"
            >
              <Edit class="w-3.5 h-3.5" />
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              class="h-8 w-8"
              title="删除"
              :disabled="deletingId === record.cost_id"
              @click="removeRecord(record)"
            >
              <Trash2 class="w-3.5 h-3.5" />
            </Button>
          </div>
        </div>
      </div>

      <!-- 编辑表单 -->
      <div
        v-if="editing"
        class="space-y-3 rounded-lg border p-3"
      >
        <p class="text-sm font-medium">
          {{ editing.costId ? '编辑价目' : '新增价目' }}
        </p>
        <div class="grid grid-cols-2 gap-3">
          <div class="space-y-1">
            <Label class="text-xs">任务类型</Label>
            <select
              v-model="editing.taskType"
              class="flex h-9 w-full rounded-md border border-input bg-background px-3 text-sm"
            >
              <option value="text">
                文本
              </option>
              <option value="image">
                图像
              </option>
            </select>
          </div>
          <div class="space-y-1">
            <Label class="text-xs">币种</Label>
            <Input
              v-model="editing.currency"
              placeholder="USD"
            />
          </div>
          <div class="space-y-1">
            <Label class="text-xs">按次成本(留空表示不按次)</Label>
            <Input
              v-model="editing.pricePerRequest"
              type="number"
              min="0"
              step="any"
              placeholder="例如 0.01"
            />
          </div>
          <div class="space-y-1">
            <Label class="text-xs">生效开始(unix 秒)</Label>
            <Input
              v-model="editing.effectiveFrom"
              type="number"
              min="0"
              step="1"
            />
          </div>
        </div>
        <div class="space-y-1">
          <Label class="text-xs">生效结束(unix 秒,留空表示长期有效)</Label>
          <Input
            v-model="editing.effectiveTo"
            type="number"
            min="0"
            step="1"
            placeholder="留空表示长期有效"
          />
        </div>
        <div class="space-y-1">
          <Label class="text-xs">分档 catalog(JSON,形状与售价 tiered_pricing 一致;与按次成本至少填一项)</Label>
          <Textarea
            v-model="editing.tieredPricing"
            rows="8"
            class="font-mono text-xs"
            placeholder='{"tiers":[{"input_price_per_1m":0.5,"output_price_per_1m":1.5}]}'
          />
        </div>
        <p
          v-if="formError"
          class="text-sm text-destructive"
        >
          {{ formError }}
        </p>
        <div class="flex justify-end gap-2">
          <Button
            type="button"
            variant="outline"
            size="sm"
            @click="editing = null"
          >
            取消
          </Button>
          <Button
            type="button"
            size="sm"
            :disabled="saving"
            @click="save"
          >
            {{ saving ? '保存中...' : '保存' }}
          </Button>
        </div>
      </div>
    </div>
  </Dialog>
</template>

<script setup lang="ts">
import { ref, watch } from 'vue'
import { Coins, Edit, Plus, Trash2 } from 'lucide-vue-next'
import { Dialog, Button, Input, Label, Textarea } from '@/components/ui'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { parseApiError } from '@/utils/errorParser'
import {
  createProviderCostCatalog,
  deleteProviderCostCatalog,
  listProviderCostCatalogs,
  updateProviderCostCatalog,
  type ProviderCostCatalogRecord,
  type ProviderCostTaskType,
} from '@/api/provider-costs'

const props = defineProps<{
  open: boolean
  providerId: string
  modelName: string
}>()

const emit = defineEmits<{
  'update:open': [value: boolean]
}>()

const { error: showError, success: showSuccess } = useToast()
const { confirmDanger } = useConfirm()

const loading = ref(false)
const loadError = ref('')
const records = ref<ProviderCostCatalogRecord[]>([])
const saving = ref(false)
const deletingId = ref('')
const formError = ref('')
const editing = ref<{
  costId: string
  taskType: ProviderCostTaskType
  currency: string
  pricePerRequest: string
  tieredPricing: string
  effectiveFrom: string
  effectiveTo: string
} | null>(null)

function handleClose(value: boolean) {
  if (!value) {
    editing.value = null
  }
  emit('update:open', value)
}

function taskTypeLabel(taskType: ProviderCostTaskType): string {
  return taskType === 'image' ? '图像' : '文本'
}

function formatWindow(record: ProviderCostCatalogRecord): string {
  const from = new Date(record.effective_from_unix_secs * 1000).toLocaleString()
  if (record.effective_to_unix_secs == null) {
    return `${from} 起长期有效`
  }
  const to = new Date(record.effective_to_unix_secs * 1000).toLocaleString()
  return `${from} ~ ${to}`
}

async function load() {
  if (!props.open || !props.providerId || !props.modelName) {
    return
  }
  loading.value = true
  loadError.value = ''
  try {
    records.value = await listProviderCostCatalogs({
      provider_id: props.providerId,
      model: props.modelName,
      page_size: 200,
    })
  } catch (error) {
    records.value = []
    loadError.value = parseApiError(error, '成本价目加载失败')
  } finally {
    loading.value = false
  }
}

watch(() => [props.open, props.providerId, props.modelName], load, { immediate: true })

function startCreate() {
  formError.value = ''
  editing.value = {
    costId: '',
    taskType: 'text',
    currency: 'USD',
    pricePerRequest: '',
    tieredPricing: '',
    effectiveFrom: String(Math.floor(Date.now() / 1000)),
    effectiveTo: '',
  }
}

function startEdit(record: ProviderCostCatalogRecord) {
  formError.value = ''
  editing.value = {
    costId: record.cost_id,
    taskType: record.task_type,
    currency: record.currency,
    pricePerRequest: record.price_per_request == null ? '' : String(record.price_per_request),
    tieredPricing: record.tiered_pricing ? JSON.stringify(record.tiered_pricing, null, 2) : '',
    effectiveFrom: String(record.effective_from_unix_secs),
    effectiveTo: record.effective_to_unix_secs == null ? '' : String(record.effective_to_unix_secs),
  }
}

function parseForm(): {
  payload: Parameters<typeof createProviderCostCatalog>[0]
} | { error: string } {
  const current = editing.value
  if (!current) {
    return { error: '没有正在编辑的价目' }
  }
  const pricePerRequest = current.pricePerRequest.trim()
  const tieredPricingText = current.tieredPricing.trim()
  if (!pricePerRequest && !tieredPricingText) {
    return { error: '按次成本与分档 catalog 至少填写一项' }
  }
  let tieredPricing: Record<string, unknown> | null = null
  if (tieredPricingText) {
    try {
      const parsed: unknown = JSON.parse(tieredPricingText)
      if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
        return { error: '分档 catalog 必须是 JSON 对象' }
      }
      tieredPricing = parsed as Record<string, unknown>
    } catch {
      return { error: '分档 catalog 不是合法 JSON' }
    }
  }
  const price = pricePerRequest ? Number(pricePerRequest) : null
  if (price != null && (!Number.isFinite(price) || price < 0)) {
    return { error: '按次成本必须是非负数字' }
  }
  const effectiveFrom = Number(current.effectiveFrom)
  if (!Number.isInteger(effectiveFrom) || effectiveFrom < 0) {
    return { error: '生效开始必须是有效的 unix 秒' }
  }
  const effectiveToText = current.effectiveTo.trim()
  let effectiveTo: number | null = null
  if (effectiveToText) {
    effectiveTo = Number(effectiveToText)
    if (!Number.isInteger(effectiveTo) || effectiveTo <= effectiveFrom) {
      return { error: '生效结束必须是大于生效开始的 unix 秒' }
    }
  }
  return {
    payload: {
      provider_id: props.providerId,
      model: props.modelName,
      task_type: current.taskType,
      currency: current.currency.trim() || 'USD',
      price_per_request: price,
      tiered_pricing: tieredPricing,
      effective_from_unix_secs: effectiveFrom,
      effective_to_unix_secs: effectiveTo,
    },
  }
}

async function save() {
  const current = editing.value
  if (!current) {
    return
  }
  const parsed = parseForm()
  if ('error' in parsed) {
    formError.value = parsed.error
    return
  }
  formError.value = ''
  saving.value = true
  try {
    if (current.costId) {
      await updateProviderCostCatalog(current.costId, parsed.payload)
    } else {
      await createProviderCostCatalog(parsed.payload)
    }
    showSuccess('成本价目已保存')
    editing.value = null
    await load()
  } catch (error) {
    formError.value = parseApiError(error, '成本价目保存失败')
  } finally {
    saving.value = false
  }
}

async function removeRecord(record: ProviderCostCatalogRecord) {
  const confirmed = await confirmDanger(`确认删除 ${taskTypeLabel(record.task_type)} 价目?该操作不可恢复。`, '删除成本价目')
  if (!confirmed) {
    return
  }
  deletingId.value = record.cost_id
  try {
    await deleteProviderCostCatalog(record.cost_id)
    showSuccess('成本价目已删除')
    await load()
  } catch (error) {
    showError(parseApiError(error, '成本价目删除失败'))
  } finally {
    deletingId.value = ''
  }
}
</script>
