import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, type App } from 'vue'

const { getUsageMarginStatsMock } = vi.hoisted(() => ({
  getUsageMarginStatsMock: vi.fn(),
}))

vi.mock('@/api/usage', () => ({
  usageApi: {
    getUsageMarginStats: getUsageMarginStatsMock,
  },
}))

vi.mock('@/components/common', () => ({
  TimeRangePicker: {
    name: 'TimeRangePicker',
    template: '<div data-test="time-range-picker" />',
  },
}))

import MarginReport from '../MarginReport.vue'

function marginRow(overrides: Record<string, unknown> = {}) {
  return {
    period_start: '2026-09-01',
    model: 'gpt-5',
    provider_id: 'provider-openai',
    request_count: 10,
    revenue: '2.00000000',
    cost: '1.50000000',
    margin: '0.50000000',
    margin_rate: 25,
    currency: 'USD',
    cost_coverage: {
      estimated_requests: 10,
      known_requests: 0,
      unknown_requests: 0,
      total_requests: 10,
      estimated_share_percent: 100,
    },
    ...overrides,
  }
}

describe('MarginReport.vue', () => {
  let app: App | null = null
  let root: HTMLDivElement | null = null

  beforeEach(() => {
    getUsageMarginStatsMock.mockReset()
    getUsageMarginStatsMock.mockResolvedValue([marginRow()])
    root = document.createElement('div')
    document.body.appendChild(root)
  })

  afterEach(() => {
    app?.unmount()
    app = null
    root?.remove()
    root = null
  })

  async function mountReport() {
    app = createApp(defineComponent({
      setup: () => () => h(MarginReport),
    }))
    app.mount(root!)
    await nextTick()
    await nextTick()
  }

  it('loads and renders margin rows for the selected range', async () => {
    await mountReport()

    expect(getUsageMarginStatsMock).toHaveBeenCalledWith(
      expect.objectContaining({ granularity: 'day' }),
      { skipCache: true }
    )
    expect(root!.textContent).toContain('毛利报表')
    expect(root!.textContent).toContain('gpt-5')
    expect(root!.textContent).toContain('0.50000000')
  })

  it('marks rows with unknown cost coverage as unknown instead of zero margin', async () => {
    getUsageMarginStatsMock.mockResolvedValue([
      marginRow({
        margin: null,
        margin_rate: null,
        cost_coverage: {
          estimated_requests: 5,
          known_requests: 0,
          unknown_requests: 5,
          total_requests: 10,
          estimated_share_percent: 50,
        },
      }),
    ])
    await mountReport()

    expect(root!.textContent).toContain('未知')
    expect(root!.textContent).toContain('存在未知成本')
    expect(root!.textContent).not.toContain('0.50000000')
  })

  it('renders an empty state when no margin rows exist', async () => {
    getUsageMarginStatsMock.mockResolvedValue([])
    await mountReport()

    expect(root!.textContent).toContain('当前时间范围内暂无毛利数据')
  })
})
