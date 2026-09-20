import { beforeEach, describe, expect, it, vi } from 'vitest'

const { getMock } = vi.hoisted(() => ({ getMock: vi.fn() }))
vi.mock('@/api/client', () => ({ default: { get: getMock } }))
import { walletApi } from '../wallet'

describe('current-user recharge recoveries', () => {
  beforeEach(() => getMock.mockReset())

  it('uses the authenticated wallet endpoint without user or wallet selectors', async () => {
    const response = { items: [], limit: 50 }
    getMock.mockResolvedValue({ data: response })
    expect(await walletApi.listRechargeRecoveries()).toEqual(response)
    expect(getMock).toHaveBeenCalledExactlyOnceWith(
      '/api/wallet/recharge-recoveries', { params: { limit: 50 } },
    )
  })
})
