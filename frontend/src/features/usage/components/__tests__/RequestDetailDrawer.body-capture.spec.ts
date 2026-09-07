import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, ref, type App } from 'vue'

import type { RequestDetail } from '@/api/dashboard'
import RequestDetailDrawer from '../RequestDetailDrawer.vue'

const apiMocks = vi.hoisted(() => ({ getRequestDetail: vi.fn() }))

vi.mock('@/api/dashboard', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/api/dashboard')>()
  return {
    ...actual,
    dashboardApi: { ...actual.dashboardApi, getRequestDetail: apiMocks.getRequestDetail },
  }
})

vi.mock('../HorizontalRequestTimeline.vue', () => ({ default: { render: () => null } }))

vi.mock('../JsonContentPanel.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      props: { data: { type: null, default: null } },
      setup(props) {
        return () => h('pre', { 'data-testid': 'captured-body' }, JSON.stringify(props.data))
      },
    }),
  }
})

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
  apiMocks.getRequestDetail.mockReset()
})

function buildDetail(captured: boolean): RequestDetail {
  return {
    id: 'usage-full-capture',
    request_id: 'req-full-capture',
    user: { id: 'user-1', username: 'test-user', email: 'test@example.com' },
    api_key: { id: 'key-1', name: 'test-key', display: 'test-key' },
    provider: 'test-provider',
    api_format: 'openai:chat',
    model: 'test-model',
    tokens: { input: 10, output: 20, total: 30 },
    cost: { input: 0, output: 0, total: 0 },
    request_type: 'chat',
    is_stream: false,
    status: 'completed',
    status_code: 200,
    response_time_ms: 10,
    created_at: '2026-09-07T00:00:00Z',
    request_headers: { 'content-type': 'application/json', authorization: '[redacted]' },
    has_request_body: captured,
    has_provider_request_body: false,
    has_response_body: captured,
    has_client_response_body: false,
  }
}

async function openDrawer() {
  const isOpen = ref(false)
  const Host = defineComponent({
    setup: () => () => h(RequestDetailDrawer, {
      isOpen: isOpen.value,
      requestId: 'usage-full-capture',
    }),
  })
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(Host)
  app.mount(root)
  mountedApps.push({ app, root })
  isOpen.value = true
  await nextTick()
  await vi.waitFor(() => {
    expect(document.body.textContent).toContain('请求头')
  })
}

function findTab(label: string) {
  return [...document.body.querySelectorAll('button')]
    .find(button => button.textContent?.trim() === label)
}

describe('RequestDetailDrawer body capture', () => {
  it('keeps body tabs from shallow availability and loads full captures on demand', async () => {
    const shallow = buildDetail(true)
    const full: RequestDetail = {
      ...shallow,
      request_body: { messages: [{ role: 'user', content: 'captured request text' }] },
      response_body: { choices: [{ message: { role: 'assistant', content: 'captured response text' } }] },
    }
    apiMocks.getRequestDetail.mockImplementation(async (_requestId, options) => (
      options?.includeBodies ? full : shallow
    ))
    await openDrawer()

    expect(findTab('请求体')).toBeDefined()
    expect(findTab('响应体')).toBeDefined()
    expect(apiMocks.getRequestDetail).toHaveBeenCalledTimes(1)
    expect(apiMocks.getRequestDetail).toHaveBeenCalledWith('usage-full-capture', expect.objectContaining({ includeBodies: false }))

    findTab('请求体')!.click()
    await vi.waitFor(() => {
      expect(document.body.querySelector('[data-testid="captured-body"]')?.textContent)
        .toContain('captured request text')
    })
    expect(apiMocks.getRequestDetail).toHaveBeenLastCalledWith('usage-full-capture', { includeBodies: true })

    findTab('响应体')!.click()
    await nextTick()
    expect(document.body.querySelector('[data-testid="captured-body"]')?.textContent)
      .toContain('captured response text')
    expect(apiMocks.getRequestDetail).toHaveBeenCalledTimes(2)
  })

  it('does not offer body tabs or fetch uncaptured bodies for basic records', async () => {
    apiMocks.getRequestDetail.mockResolvedValue(buildDetail(false))
    await openDrawer()
    expect(findTab('请求体')).toBeUndefined()
    expect(findTab('响应体')).toBeUndefined()
    expect(apiMocks.getRequestDetail).toHaveBeenCalledTimes(1)
  })
})
