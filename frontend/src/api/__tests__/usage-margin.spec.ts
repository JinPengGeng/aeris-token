import { beforeEach, describe, expect, it, vi } from 'vitest'

const { getMock, cachedRequestMock } = vi.hoisted(() => ({
  getMock: vi.fn(),
  cachedRequestMock: vi.fn(async (_key: string, fn: () => Promise<unknown>) => fn()),
}))

vi.mock('@/api/client', () => ({
  default: {
    get: getMock,
  },
}))

vi.mock('@/utils/cache', () => ({
  cachedRequest: cachedRequestMock,
  dedupedRequest: vi.fn(async (_key: string, fn: () => Promise<unknown>) => fn()),
  buildCacheKey: vi.fn(() => 'cache-key'),
}))

import { usageApi } from '@/api/usage'

describe('usageApi margin report contract', () => {
  beforeEach(() => {
    getMock.mockReset()
    cachedRequestMock.mockClear()
  })

  it('loads margin stats from the admin margin endpoint with filters', async () => {
    getMock.mockResolvedValueOnce({
      data: [
        {
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
            estimated_share_percent: 100
          }
        }
      ]
    })

    const result = await usageApi.getUsageMarginStats(
      { start_date: '2026-09-01', end_date: '2026-09-24', granularity: 'day', limit: 500 },
      { skipCache: true }
    )

    expect(getMock).toHaveBeenCalledWith('/api/admin/usage/margin/stats', {
      params: { start_date: '2026-09-01', end_date: '2026-09-24', granularity: 'day', limit: 500 },
      timeout: expect.any(Number)
    })
    expect(result).toHaveLength(1)
    expect(result[0].margin).toBe('0.50000000')
    expect(result[0].cost_coverage.estimated_requests).toBe(10)
  })

  it('propagates null margin for rows with unknown cost coverage', async () => {
    getMock.mockResolvedValueOnce({
      data: [
        {
          period_start: '2026-09-01',
          model: 'gpt-5',
          provider_id: 'provider-openai',
          request_count: 10,
          revenue: '2.00000000',
          cost: '1.00000000',
          margin: null,
          margin_rate: null,
          currency: 'USD',
          cost_coverage: {
            estimated_requests: 5,
            known_requests: 0,
            unknown_requests: 5,
            total_requests: 10,
            estimated_share_percent: 50
          }
        }
      ]
    })

    const result = await usageApi.getUsageMarginStats({ granularity: 'week' })

    expect(result[0].margin).toBeNull()
    expect(result[0].margin_rate).toBeNull()
    expect(result[0].cost_coverage.unknown_requests).toBe(5)
  })
})
