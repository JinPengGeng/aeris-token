import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, ref, type App } from 'vue'
import CurrentAuthorizationPanel from '../CurrentAuthorizationPanel.vue'

const mocks = vi.hoisted(() => ({ get: vi.fn() }))
const role = ref('admin')
vi.mock('@/stores/auth', () => ({ useAuthStore: () => ({ get isAdmin() { return role.value === 'admin' } }) }))
vi.mock('@/api/requestAuth', () => ({ requestAuthApi: { getCurrentAuthorization: mocks.get } }))
const mounted: Array<{ app: App, root: HTMLElement }> = []
const identity = { user_id: 'caller', api_key_id: 'caller-key' }

beforeEach(() => {
  role.value = 'admin'
  mocks.get.mockResolvedValue(identity)
})
afterEach(() => {
  for (const { app, root } of mounted.splice(0)) { app.unmount(); root.remove() }
  mocks.get.mockReset()
})

function mountPanel(userId: string | null = 'caller', apiKeyId: string | null = 'caller-key') {
  const isOpen = ref(true)
  const requestId = ref('usage-first')
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(defineComponent({ setup: () => () => h(CurrentAuthorizationPanel, {
    isOpen: isOpen.value, requestId: requestId.value, userId, apiKeyId,
  }) }))
  app.mount(root)
  mounted.push({ app, root })
  const load = () => root.querySelector<HTMLButtonElement>('[data-testid="load-current-authorization"]')?.click()
  return { root, isOpen, requestId, load }
}

describe('CurrentAuthorizationPanel access and display', () => {
  it('displays only selected policy fields and preserves partial information as unknown', async () => {
    mocks.get.mockResolvedValue({
      ...identity, user_is_active: true, api_key_is_active: false,
      user_allowed_models: ['user-model'], api_key_allowed_models: ['key-model'],
      email: 'private-email@example.com', api_key_ip_rules: ['private-network-marker'],
      secret: 'private-secret-marker', request_body: { text: 'private-body-marker' },
    })
    const host = mountPanel()
    expect(mocks.get).not.toHaveBeenCalled()
    host.load()
    await vi.waitFor(() => expect(host.root.querySelector('[data-testid="authorization-snapshot"]')).not.toBeNull())
    expect(host.root.textContent).toContain('活跃')
    expect(host.root.textContent).toContain('停用')
    expect(host.root.textContent).toContain('未知')
    expect(host.root.textContent).toContain('未提供')
    expect(host.root.textContent).toContain('user-model')
    expect(host.root.textContent).toContain('key-model')
    expect(host.root.textContent).not.toContain('private-')
  })

  it.each(['audit_admin', 'user'])('does not expose or request policy for %s', deniedRole => {
    role.value = deniedRole
    const host = mountPanel()
    host.load()
    expect(host.root.querySelector('button')).toBeNull()
    expect(mocks.get).not.toHaveBeenCalled()
  })

  it.each([[null, 'key'], ['user', null], [null, null]])('explains missing IDs without issuing a request (%s / %s)', (userId, apiKeyId) => {
    const host = mountPanel(userId, apiKeyId)
    host.load()
    expect(host.root.textContent).toContain('缺少调用方用户或 API Key 标识')
    expect(host.root.querySelector<HTMLButtonElement>('button')?.disabled).toBe(true)
    expect(mocks.get).not.toHaveBeenCalled()
  })

  it.each([
    [403, '你没有权限查看该授权状态'],
    [404, '未找到该用户或 API Key'],
    [undefined, '查询授权状态失败'],
  ])('shows a safe error and supports retry (%s)', async (status, message) => {
    mocks.get.mockRejectedValueOnce({ response: { status, data: { detail: 'secret-server-detail' } } })
    const host = mountPanel()
    host.load()
    await vi.waitFor(() => expect(host.root.textContent).toContain(message))
    expect(host.root.textContent).not.toContain('secret-server-detail')
    ;[...host.root.querySelectorAll('button')].find(button => button.textContent?.trim() === '重试')!.click()
    await vi.waitFor(() => expect(host.root.querySelector('[data-testid="authorization-snapshot"]')).not.toBeNull())
    expect(mocks.get).toHaveBeenCalledTimes(2)
  })

  it('rejects a response for a different identity', async () => {
    mocks.get.mockResolvedValue({ user_id: 'other', api_key_id: 'caller-key', user_allowed_models: ['wrong-model'] })
    const host = mountPanel()
    host.load()
    await vi.waitFor(() => expect(host.root.textContent).toContain('授权身份与查询目标不一致'))
    expect(host.root.textContent).not.toContain('wrong-model')
  })

  it.each(['request', 'close', 'role'] as const)('ignores late responses after %s changes', async change => {
    let resolve!: (data: Record<string, unknown>) => void
    mocks.get.mockImplementationOnce(() => new Promise(done => { resolve = done }))
    const host = mountPanel()
    host.load()
    const signal = mocks.get.mock.calls[0][2] as AbortSignal
    if (change === 'request') host.requestId.value = 'usage-second'
    else if (change === 'close') host.isOpen.value = false
    else role.value = 'audit_admin'
    await nextTick()
    expect(signal.aborted).toBe(true)
    resolve({ ...identity, user_allowed_models: ['stale-model'] })
    await nextTick()
    expect(host.root.textContent).not.toContain('stale-model')
    expect(host.root.querySelector('[data-testid="authorization-snapshot"]')).toBeNull()
  })
})
