import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, nextTick, type App } from 'vue'
import UserFormDialog from '../UserFormDialog.vue'

vi.mock('@/api/admin', () => ({
  adminApi: { getSystemConfig: vi.fn().mockResolvedValue({ value: 'weak' }) },
}))
vi.mock('@/components/common', async () => {
  const { defineComponent, h } = await import('vue')
  return { MultiSelect: defineComponent({ setup: () => () => h('div') }) }
})
vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')
  const passthrough = defineComponent({ setup: (_, { slots }) => () => h('div', slots.default?.()) })
  return {
    ...Object.fromEntries([
      'Label', 'Select', 'SelectTrigger', 'SelectValue', 'SelectContent', 'SelectItem',
    ].map(name => [name, passthrough])),
    Dialog: defineComponent({
      setup: (_, { slots }) => () => h('div', [slots.header?.(), slots.default?.(), slots.footer?.()]),
    }),
    Button: defineComponent({ setup: (_, { slots }) => () => h('button', slots.default?.()) }),
    Input: defineComponent({
      props: { modelValue: [String, Number] },
      emits: ['update:modelValue'],
      setup: (props, { emit }) => () => h('input', {
        value: props.modelValue,
        onInput: (event: Event) => emit('update:modelValue', (event.target as HTMLInputElement).value),
      }),
    }),
    Switch: defineComponent({
      props: { modelValue: Boolean },
      emits: ['update:modelValue'],
      setup: (props, { emit }) => () => h('button', {
        type: 'button',
        'data-switch': true,
        onClick: () => emit('update:modelValue', !props.modelValue),
      }),
    }),
  }
})

const mounted: Array<{ app: App; root: HTMLElement }> = []

afterEach(() => {
  for (const { app, root } of mounted.splice(0)) {
    app.unmount()
    root.remove()
  }
})

async function setInput(root: HTMLElement, selector: string, value: string) {
  const input = root.querySelector<HTMLInputElement>(selector)!
  input.value = value
  input.dispatchEvent(new Event('input', { bubbles: true }))
  await nextTick()
}

async function mountNewUser() {
  const root = document.createElement('div')
  document.body.append(root)
  const submit = vi.fn()
  const app = createApp(UserFormDialog, { open: true, user: null, onSubmit: submit })
  app.mount(root)
  mounted.push({ app, root })
  await nextTick()
  await setInput(root, '#form-username', 'new_user')
  await setInput(root, 'input[id^="pwd-"]', 'secret123')
  const createButton = [...root.querySelectorAll<HTMLButtonElement>('button')]
    .find(button => button.textContent === '创建')!
  return { root, submit, createButton }
}

describe('UserFormDialog initial gift', () => {
  it('allows creating an account with zero credit, including after toggling unlimited', async () => {
    const { root, submit, createButton } = await mountNewUser()
    const gift = () => root.querySelector<HTMLInputElement>('#form-initial-gift')!
    expect(gift().value).toBe('0')
    expect(gift().min).toBe('0')
    expect(createButton.disabled).toBe(false)

    const unlimited = root.querySelector<HTMLButtonElement>('[data-switch]')!
    unlimited.click()
    await nextTick()
    expect(gift()).toBeNull()
    unlimited.click()
    await nextTick()
    expect(gift().value).toBe('0')
    expect(createButton.disabled).toBe(false)
    createButton.click()
    await nextTick()
    expect(submit).toHaveBeenCalledWith(expect.objectContaining({
      username: 'new_user', initial_gift_usd: 0, unlimited: false,
    }))
  })

  it('keeps an explicitly entered gift in the submitted request', async () => {
    const { root, submit, createButton } = await mountNewUser()
    await setInput(root, '#form-initial-gift', '6.5')
    expect(createButton.disabled).toBe(false)
    createButton.click()
    await nextTick()
    expect(submit).toHaveBeenCalledWith(expect.objectContaining({ initial_gift_usd: 6.5 }))
  })
})
