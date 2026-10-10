import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import type { RouteLocationNormalized } from 'vue-router'

const { getAllStatusMock, warnMock } = vi.hoisted(() => ({
  getAllStatusMock: vi.fn(),
  warnMock: vi.fn(),
}))

vi.mock('@/api/modules', () => ({
  modulesApi: {
    getAllStatus: getAllStatusMock,
  },
}))

vi.mock('@/utils/logger', () => ({
  log: {
    debug: vi.fn(),
    info: vi.fn(),
    warn: warnMock,
    error: vi.fn(),
  },
}))

import { checkModuleAccess } from '@/router/guards/moduleGuard'
import { useModuleStore } from '@/stores/modules'
import type { ModuleStatus } from '@/api/modules'

function routeWithModuleMeta(module: string | undefined): RouteLocationNormalized {
  return {
    path: '/dashboard/referral',
    fullPath: '/dashboard/referral',
    query: {},
    hash: '',
    name: 'ReferralCenter',
    params: {},
    matched: [
      { meta: { requiresAuth: true } },
      ...(module === undefined ? [] : [{ meta: { module } }]),
    ],
    meta: {},
    redirectedFrom: undefined,
  } as unknown as RouteLocationNormalized
}

function moduleStatus(active: boolean): ModuleStatus {
  return {
    name: 'referral',
    available: active,
    enabled: active,
    active,
    config_validated: active,
    config_error: null,
    display_name: 'referral',
    description: '',
    category: 'integration',
    admin_route: null,
    admin_menu_icon: null,
    admin_menu_group: null,
    admin_menu_order: 0,
    health: 'unknown',
  }
}

describe('checkModuleAccess', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    getAllStatusMock.mockReset()
    warnMock.mockReset()
  })

  it('放行没有 meta.module 的路由且不触发模块状态加载', async () => {
    const moduleStore = useModuleStore()

    await expect(checkModuleAccess(routeWithModuleMeta(undefined), moduleStore)).resolves.toBeNull()
    expect(getAllStatusMock).not.toHaveBeenCalled()
  })

  it('模块状态加载失败时 fail-close 重定向到 /dashboard', async () => {
    const moduleStore = useModuleStore()
    getAllStatusMock.mockRejectedValue(new Error('status 403'))

    await expect(
      checkModuleAccess(routeWithModuleMeta('referral'), moduleStore)
    ).resolves.toBe('/dashboard')
    expect(warnMock).toHaveBeenCalled()
  })

  it('模块未激活时重定向到 /dashboard', async () => {
    const moduleStore = useModuleStore()
    getAllStatusMock.mockResolvedValue({ referral: moduleStatus(false) })

    await expect(
      checkModuleAccess(routeWithModuleMeta('referral'), moduleStore)
    ).resolves.toBe('/dashboard')
  })

  it('模块已激活时放行并复用已加载的模块状态', async () => {
    const moduleStore = useModuleStore()
    getAllStatusMock.mockResolvedValue({ referral: moduleStatus(true) })

    await expect(
      checkModuleAccess(routeWithModuleMeta('referral'), moduleStore)
    ).resolves.toBeNull()
    expect(moduleStore.loaded).toBe(true)

    getAllStatusMock.mockClear()
    await expect(
      checkModuleAccess(routeWithModuleMeta('referral'), moduleStore)
    ).resolves.toBeNull()
    expect(getAllStatusMock).not.toHaveBeenCalled()
  })
})
