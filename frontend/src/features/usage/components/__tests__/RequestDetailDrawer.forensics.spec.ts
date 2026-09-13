import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, ref, type App } from 'vue'
import type { RequestDetail } from '@/api/dashboard'
import RequestDetailDrawer from '../RequestDetailDrawer.vue'

const mocks = vi.hoisted(() => ({ detail: vi.fn(), trace: vi.fn(), auth: vi.fn() }))
const role = ref('admin')
vi.mock('@/stores/auth', () => ({ useAuthStore: () => ({ get isAdmin() { return role.value === 'admin' } }) }))
vi.mock('@/api/dashboard', async importOriginal => {
  const actual = await importOriginal<typeof import('@/api/dashboard')>()
  return { ...actual, dashboardApi: { ...actual.dashboardApi, getRequestDetail: mocks.detail } }
})
vi.mock('@/api/requestTrace', () => ({ requestTraceApi: { getRequestTrace: mocks.trace } }))
vi.mock('@/api/requestAuth', () => ({ requestAuthApi: { getCurrentAuthorization: mocks.auth } }))

const mounted: Array<{ app: App, root: HTMLElement }> = []
const detail = (id: string): RequestDetail => ({
  id, request_id: `request-${id}`,
  user: { id: 'actual-user', username: 'caller', email: '' },
  api_key: { id: 'actual-key', name: 'caller-key', display: 'caller-key' },
  provider: 'test', model: 'test', request_type: 'chat', is_stream: false,
  status: 'completed', status_code: 200, tokens: { input: 1, output: 1, total: 2 },
  cost: { input: 0, output: 0, total: 0 }, response_time_ms: 20,
  created_at: '2026-09-13T00:00:00Z', trace: { trace_id: `trace-${id}` },
})

beforeEach(() => {
  role.value = 'admin'
  mocks.detail.mockImplementation(async id => detail(id))
  mocks.trace.mockImplementation(async id => ({ request_id: id, candidates: [], total_candidates: 0, final_status: 'success', total_latency_ms: 0 }))
  mocks.auth.mockResolvedValue({ user_id: 'actual-user', api_key_id: 'actual-key', currently_usable: true })
})
afterEach(() => {
  for (const { app, root } of mounted.splice(0)) { app.unmount(); root.remove() }
  document.body.replaceChildren()
  vi.resetAllMocks()
})

const button = (label: string) => [...document.body.querySelectorAll('button')].find(item => item.textContent?.trim() === label)
async function openDrawer() {
  const isOpen = ref(false)
  const requestId = ref('usage-first')
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(defineComponent({ setup: () => () => h(RequestDetailDrawer, { isOpen: isOpen.value, requestId: requestId.value }) }))
  app.mount(root)
  mounted.push({ app, root })
  isOpen.value = true
  await vi.waitFor(() => expect(mocks.trace).toHaveBeenCalled())
  return { isOpen, requestId }
}

describe('RequestDetailDrawer forensic access', () => {
  it('loads all candidates and the caller policy only on explicit admin actions', async () => {
    await openDrawer()
    expect(mocks.trace).toHaveBeenLastCalledWith('trace-usage-first', expect.objectContaining({ attemptedOnly: true }))
    expect(mocks.auth).not.toHaveBeenCalled()
    button('全部候选')!.click()
    await vi.waitFor(() => expect(mocks.trace).toHaveBeenLastCalledWith('trace-usage-first', expect.objectContaining({ attemptedOnly: false })))
    expect(document.body.textContent).toContain('不代表所有候选都向上游发起了请求')
    button('查看当前授权状态')!.click()
    await vi.waitFor(() => expect(mocks.auth).toHaveBeenCalledWith('actual-user', 'actual-key', expect.any(AbortSignal)))
    expect(document.body.textContent).toContain('不代表请求发生时的历史权限')
  })

  it.each(['audit_admin', 'user'])('never starts exclusive reads for %s', async deniedRole => {
    role.value = deniedRole
    await openDrawer()
    expect(button('全部候选')).toBeUndefined()
    expect(button('查看当前授权状态')).toBeUndefined()
    expect(mocks.auth).not.toHaveBeenCalled()
    expect(mocks.trace.mock.calls.every(([, options]) => options.attemptedOnly === true)).toBe(true)
  })

  it('cancels pending exclusive reads and clears the selected scope on downgrade', async () => {
    await openDrawer()
    mocks.trace.mockImplementationOnce(() => new Promise(() => undefined))
    mocks.auth.mockImplementationOnce(() => new Promise(() => undefined))
    button('全部候选')!.click()
    button('查看当前授权状态')!.click()
    await vi.waitFor(() => expect(mocks.trace).toHaveBeenCalledTimes(2))
    const traceSignal = mocks.trace.mock.calls[1][1].signal as AbortSignal
    const authSignal = mocks.auth.mock.calls[0][2] as AbortSignal
    role.value = 'audit_admin'
    await nextTick()
    expect(traceSignal.aborted).toBe(true)
    expect(authSignal.aborted).toBe(true)
    expect(button('全部候选')).toBeUndefined()
    expect(document.body.querySelector('[data-testid="current-authorization-panel"]')).toBeNull()
    role.value = 'admin'
    await nextTick()
    expect(button('已尝试候选')?.getAttribute('aria-pressed')).toBe('true')
    expect(mocks.auth).toHaveBeenCalledTimes(1)
  })

  it.each(['close', 'request'] as const)('cancels exclusive reads on %s even when the caller IDs stay the same', async action => {
    const host = await openDrawer()
    mocks.trace.mockImplementationOnce(() => new Promise(() => undefined))
    mocks.auth.mockImplementationOnce(() => new Promise(() => undefined))
    button('全部候选')!.click()
    button('查看当前授权状态')!.click()
    await vi.waitFor(() => expect(mocks.trace).toHaveBeenCalledTimes(2))
    const traceSignal = mocks.trace.mock.calls[1][1].signal as AbortSignal
    const authSignal = mocks.auth.mock.calls[0][2] as AbortSignal
    if (action === 'close') host.isOpen.value = false
    else host.requestId.value = 'usage-second'
    await vi.waitFor(() => {
      expect(traceSignal.aborted).toBe(true)
      expect(authSignal.aborted).toBe(true)
    })
    if (action === 'request') {
      await vi.waitFor(() => expect(mocks.trace).toHaveBeenLastCalledWith('trace-usage-second', expect.objectContaining({ attemptedOnly: true })))
      expect(button('已尝试候选')?.getAttribute('aria-pressed')).toBe('true')
      expect(mocks.auth).toHaveBeenCalledTimes(1)
    }
  })
})
