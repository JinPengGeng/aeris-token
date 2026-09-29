import { beforeEach, describe, expect, it, vi } from 'vitest'

const { getMock, authState } = vi.hoisted(() => ({
  getMock: vi.fn(),
  authState: { canAccessAdmin: false },
}))

vi.mock('@/api/client', () => ({
  default: {
    get: getMock,
  },
}))

vi.mock('@/stores/auth', () => ({
  useAuthStore: () => authState,
}))

import { modulesApi } from '@/api/modules'

describe('modulesApi.getAllStatus role routing', () => {
  beforeEach(() => {
    getMock.mockReset()
    authState.canAccessAdmin = false
  })

  it('普通用户走用户侧只读端点并归一化为 ModuleStatus 形状', async () => {
    getMock.mockResolvedValue({
      data: {
        referral: { name: 'referral', active: true },
        oauth: { name: 'oauth', active: false },
      },
    })

    const statuses = await modulesApi.getAllStatus()

    expect(getMock).toHaveBeenCalledTimes(1)
    expect(getMock).toHaveBeenCalledWith('/api/modules/status')
    expect(statuses.referral).toMatchObject({
      name: 'referral',
      available: true,
      enabled: true,
      active: true,
      admin_route: null,
    })
    expect(statuses.oauth).toMatchObject({
      name: 'oauth',
      available: false,
      enabled: false,
      active: false,
      config_validated: false,
      config_error: null,
      admin_route: null,
    })
  })

  it('active 缺失或非布尔时按 fail-close 归一化为 false', async () => {
    getMock.mockResolvedValue({
      data: {
        referral: { name: 'referral' },
      } as never,
    })

    const statuses = await modulesApi.getAllStatus()

    expect(statuses.referral).toMatchObject({
      name: 'referral',
      active: false,
      available: false,
      enabled: false,
    })
  })

  it('管理员仍走 /api/admin/modules/status 且不做归一化', async () => {
    authState.canAccessAdmin = true
    const adminStatus = {
      name: 'referral',
      available: true,
      enabled: false,
      active: false,
      config_validated: true,
      config_error: null,
      display_name: '邀请返利',
      description: '',
      category: 'integration',
      admin_route: '/admin/referrals',
      admin_menu_icon: 'Gift',
      admin_menu_group: 'management',
      admin_menu_order: 75,
      health: 'unknown',
    }
    getMock.mockResolvedValue({ data: { referral: adminStatus } })

    const statuses = await modulesApi.getAllStatus()

    expect(getMock).toHaveBeenCalledTimes(1)
    expect(getMock).toHaveBeenCalledWith('/api/admin/modules/status')
    expect(statuses.referral).toBe(adminStatus)
  })
})
