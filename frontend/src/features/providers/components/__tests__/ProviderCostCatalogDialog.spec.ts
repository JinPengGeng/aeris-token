import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, type App } from 'vue'

import ProviderCostCatalogDialog from '../ProviderCostCatalogDialog.vue'

const apiMocks = vi.hoisted(() => ({
  listProviderCostCatalogs: vi.fn(),
  createProviderCostCatalog: vi.fn(),
  updateProviderCostCatalog: vi.fn(),
  deleteProviderCostCatalog: vi.fn(),
  findEffectiveProviderCostCatalog: vi.fn(),
}))

vi.mock('@/api/provider-costs', () => apiMocks)

vi.mock('@/composables/useToast', () => ({
  useToast: () => ({
    error: vi.fn(),
    success: vi.fn(),
    warning: vi.fn(),
  }),
}))

vi.mock('@/composables/useConfirm', () => ({
  useConfirm: () => ({
    confirmDanger: vi.fn().mockResolvedValue(true),
  }),
}))

vi.mock('@/components/ui', async () => {
  const { defineComponent } = await import('vue')
  const passthrough = (name: string) => defineComponent({
    name,
    inheritAttrs: false,
    setup: (_props, { slots }) => () => h('div', [slots.default?.()]),
  })
  const modelInput = (name: string, tag: string) => defineComponent({
    name,
    props: ['modelValue'],
    emits: ['update:modelValue'],
    setup: (props, { emit }) => () => h(tag, {
      value: props.modelValue as string,
      onInput: (event: Event) => emit('update:modelValue', (event.target as HTMLInputElement).value),
    }),
  })
  return {
    Dialog: defineComponent({
      name: 'DialogStub',
      props: ['modelValue', 'title'],
      emits: ['update:modelValue'],
      setup: (_props, { slots }) => () => h('section', [slots.default?.()]),
    }),
    Button: defineComponent({
      name: 'ButtonStub',
      inheritAttrs: false,
      setup: (_props, { attrs, slots }) => () => h('button', attrs, [slots.default?.()]),
    }),
    Input: modelInput('InputStub', 'input'),
    Textarea: modelInput('TextareaStub', 'textarea'),
    Label: passthrough('LabelStub'),
  }
})

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

async function mountDialog() {
  const wrapper = defineComponent({
    setup: () => () => h(ProviderCostCatalogDialog, {
      open: true,
      providerId: 'provider-a',
      modelName: 'gpt-x',
      'onUpdate:open': () => {},
    }),
  })
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(wrapper)
  app.mount(root)
  mountedApps.push({ app, root })
  await nextTick()
  await nextTick()
  return root
}

function record(overrides: Record<string, unknown> = {}) {
  return {
    cost_id: 'cost-a',
    provider_id: 'provider-a',
    model: 'gpt-x',
    task_type: 'text',
    currency: 'USD',
    price_per_request: null,
    tiered_pricing: { tiers: [{ input_price_per_1m: 0.5 }] },
    effective_from_unix_secs: 1_000,
    effective_to_unix_secs: null,
    created_by: 'admin-a',
    created_at_unix_secs: 900,
    updated_at_unix_secs: 900,
    ...overrides,
  }
}

describe('ProviderCostCatalogDialog', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    apiMocks.listProviderCostCatalogs.mockResolvedValue([])
  })

  afterEach(() => {
    for (const { app, root } of mountedApps.splice(0)) {
      app.unmount()
      root.remove()
    }
  })

  it('按 provider 与模型加载成本价目', async () => {
    apiMocks.listProviderCostCatalogs.mockResolvedValue([record()])
    const root = await mountDialog()

    expect(apiMocks.listProviderCostCatalogs).toHaveBeenCalledWith({
      provider_id: 'provider-a',
      model: 'gpt-x',
      page_size: 200,
    })
    expect(root.textContent).toContain('文本')
    expect(root.textContent).toContain('分档 catalog')
  })

  it('表单两项都为空时阻止保存', async () => {
    const root = await mountDialog()
    const buttons = Array.from(root.querySelectorAll('button')) as HTMLButtonElement[]
    buttons.find((button) => button.textContent?.includes('新增价目'))?.click()
    await nextTick()

    const saveButton = (Array.from(root.querySelectorAll('button')) as HTMLButtonElement[])
      .find((button) => button.textContent === '保存')
    saveButton?.click()
    await nextTick()

    expect(root.textContent).toContain('按次成本与分档 catalog 至少填写一项')
    expect(apiMocks.createProviderCostCatalog).not.toHaveBeenCalled()
  })

  it('合法表单调用创建 API 并刷新列表', async () => {
    apiMocks.createProviderCostCatalog.mockResolvedValue(undefined)
    const root = await mountDialog()
    ;(Array.from(root.querySelectorAll('button')) as HTMLButtonElement[])
      .find((button) => button.textContent?.includes('新增价目'))?.click()
    await nextTick()

    const inputs = Array.from(root.querySelectorAll('input')) as HTMLInputElement[]
    const pricing = Array.from(root.querySelectorAll('textarea')) as HTMLTextAreaElement[]
    // 表单输入顺序: 按次成本、生效开始、(生效结束)、币种
    const byPlaceholder = (value: string) => inputs.find((input) => input.placeholder === value)
    const setValue = (element: HTMLInputElement | HTMLTextAreaElement, value: string) => {
      element.value = value
      element.dispatchEvent(new Event('input'))
    }
    setValue(byPlaceholder('例如 0.01')!, '0.01')
    setValue(pricing[0], '{"tiers":[{"input_price_per_1m":0.5}]}')
    await nextTick()

    ;(Array.from(root.querySelectorAll('button')) as HTMLButtonElement[])
      .find((button) => button.textContent === '保存')?.click()
    await nextTick()

    expect(apiMocks.createProviderCostCatalog).toHaveBeenCalledWith(
      expect.objectContaining({
        provider_id: 'provider-a',
        model: 'gpt-x',
        task_type: 'text',
        price_per_request: 0.01,
        tiered_pricing: { tiers: [{ input_price_per_1m: 0.5 }] },
        effective_to_unix_secs: null,
      })
    )
    expect(apiMocks.listProviderCostCatalogs).toHaveBeenCalledTimes(2)
  })

  it('删除价目前需要确认', async () => {
    apiMocks.listProviderCostCatalogs.mockResolvedValue([record()])
    apiMocks.deleteProviderCostCatalog.mockResolvedValue(undefined)
    const root = await mountDialog()
    ;(Array.from(root.querySelectorAll('button')) as HTMLButtonElement[])
      .find((button) => button.title === '删除')?.click()
    await nextTick()
    await nextTick()

    expect(apiMocks.deleteProviderCostCatalog).toHaveBeenCalledWith('cost-a')
    expect(apiMocks.listProviderCostCatalogs).toHaveBeenCalledTimes(2)
  })
})
