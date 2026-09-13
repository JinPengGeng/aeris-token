import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, h, nextTick, ref, type App } from 'vue'

const mocks = vi.hoisted(() => ({ auth: { admin: true }, getCurrentAuthorization: vi.fn() }))
const admin = ref(true)
Object.defineProperty(mocks.auth, 'isAdmin', { get: () => admin.value })
vi.mock('@/stores/auth', () => ({ useAuthStore: () => mocks.auth }))
vi.mock('@/api/requestAuth', () => ({ requestAuthApi: { getCurrentAuthorization: mocks.getCurrentAuthorization } }))

import CurrentAuthorizationPanel from '../CurrentAuthorizationPanel.vue'

const mounted: Array<{ app: App; root: HTMLElement }> = []
afterEach(() => {
  vi.clearAllMocks()
  for (const item of mounted.splice(0)) { item.app.unmount(); item.root.remove() }
  admin.value = true
})

function mount(props: Record<string, unknown> = {}) {
  const root = document.createElement('div'); document.body.appendChild(root)
  const app = createApp({ render: () => h(CurrentAuthorizationPanel, { isOpen: true, userId: 'u1', apiKeyId: 'k1', ...props }) })
  app.mount(root); mounted.push({ app, root }); return root
}

describe('CurrentAuthorizationPanel', () => {
  it('queries only after an admin clicks and renders allowlists without sensitive fields', async () => {
    mocks.getCurrentAuthorization.mockResolvedValue({ user_id: 'u1', api_key_id: 'k1', user_is_active: true, api_key_is_active: true, currently_usable: true, user_allowed_models: ['m1'], api_key_allowed_models: ['m2'], email: 'secret@example.com', api_key_ip_rules: ['10.0.0.1'] })
    const root = mount()
    expect(mocks.getCurrentAuthorization).not.toHaveBeenCalled()
    ;(root.querySelector('[data-testid="load-current-authorization"]') as HTMLButtonElement).click()
    await nextTick(); await Promise.resolve(); await nextTick()
    expect(mocks.getCurrentAuthorization).toHaveBeenCalledWith('u1', 'k1', expect.any(AbortSignal))
    expect(root.textContent).toContain('m2'); expect(root.textContent).not.toContain('secret@example.com'); expect(root.textContent).not.toContain('10.0.0.1')
  })

  it.each([
    ['user', false], ['audit_admin', false],
  ])('%s cannot query', async (_role, allowed) => {
    admin.value = allowed
    const root = mount()
    expect(root.querySelector('[data-testid="current-authorization-panel"]')).toBeNull()
    await nextTick(); expect(mocks.getCurrentAuthorization).not.toHaveBeenCalled()
  })

  it('shows a readable message when IDs are missing', () => {
    const root = mount({ userId: null })
    expect(root.querySelector('[data-testid="authorization-missing-ids"]')?.textContent).toContain('缺少')
    expect(mocks.getCurrentAuthorization).not.toHaveBeenCalled()
  })

  it('maps authorization errors to retryable feedback', async () => {
    mocks.getCurrentAuthorization.mockRejectedValue({ response: { status: 403 } })
    const root = mount(); (root.querySelector('[data-testid="load-current-authorization"]') as HTMLButtonElement).click()
    await Promise.resolve(); await nextTick()
    expect(root.querySelector('[data-testid="authorization-error"]')?.textContent).toContain('权限')
    expect(root.querySelector('[data-testid="authorization-error"] button')).not.toBeNull()
  })

  it('aborts and ignores a late response after closing', async () => {
    let resolve!: (value: unknown) => void
    mocks.getCurrentAuthorization.mockReturnValue(new Promise((r) => { resolve = r }))
    const root = mount(); (root.querySelector('[data-testid="load-current-authorization"]') as HTMLButtonElement).click(); await nextTick()
    mounted[0].app.unmount()
    resolve({ user_id: 'u1', api_key_id: 'k1', user_is_active: true })
    await Promise.resolve(); await nextTick()
    expect(root.querySelector('[data-testid="authorization-snapshot"]')).toBeNull()
  })
})
