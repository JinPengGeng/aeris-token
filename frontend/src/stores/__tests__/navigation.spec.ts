import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'

const { getPreferencesMock, setHiddenItemsMock, errorMock } = vi.hoisted(() => ({
  getPreferencesMock: vi.fn(),
  setHiddenItemsMock: vi.fn(),
  errorMock: vi.fn(),
}))

vi.mock('@/api/navigation', () => ({
  navigationApi: {
    getPreferences: getPreferencesMock,
    setHiddenItems: setHiddenItemsMock,
  },
}))

vi.mock('@/utils/logger', () => ({
  log: {
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: errorMock,
  },
}))

vi.mock('@/utils/errorParser', () => ({
  parseApiError: (err: unknown, fallback: string) =>
    err instanceof Error ? err.message : fallback,
}))

import { useNavigationStore } from '@/stores/navigation'
import type { NavigationPreferences } from '@/api/navigation'

function preferences(overrides: Partial<NavigationPreferences> = {}): NavigationPreferences {
  return {
    hidden_items: [],
    items: [
      { key: 'operations', href: '/admin/operations', menu_group: 'overview', display_name: '运维总览' },
      { key: 'billingManagement', href: '/admin/billing-plans', menu_group: 'management', display_name: '套餐管理' },
    ],
    ...overrides,
  }
}

describe('navigation store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    getPreferencesMock.mockReset()
    setHiddenItemsMock.mockReset()
    errorMock.mockReset()
  })

  it('fetchPreferences 加载隐藏清单与项目录并复用进行中的请求', async () => {
    getPreferencesMock.mockResolvedValue(
      preferences({ hidden_items: ['billingManagement'] })
    )
    const store = useNavigationStore()

    const [first, second] = await Promise.all([
      store.fetchPreferences(),
      store.fetchPreferences(),
    ])

    expect(first.hidden_items).toEqual(['billingManagement'])
    expect(second).toBe(first)
    expect(getPreferencesMock).toHaveBeenCalledTimes(1)
    expect(store.loaded).toBe(true)
    expect(store.hiddenItems).toEqual(['billingManagement'])
    expect(store.items).toHaveLength(2)
    expect(store.isItemHidden('billingManagement')).toBe(true)
    expect(store.isItemHidden('operations')).toBe(false)
  })

  it('fetchPreferences 失败时记录错误并抛出，状态保持未加载', async () => {
    getPreferencesMock.mockRejectedValue(new Error('status 403'))
    const store = useNavigationStore()

    await expect(store.fetchPreferences()).rejects.toThrow('status 403')
    expect(errorMock).toHaveBeenCalled()
    expect(store.error).toBe('status 403')
    expect(store.loaded).toBe(false)
    expect(store.hiddenItems).toEqual([])

    // 失败后允许重试
    getPreferencesMock.mockResolvedValue(preferences())
    await expect(store.fetchPreferences()).resolves.toMatchObject({ hidden_items: [] })
    expect(store.loaded).toBe(true)
  })

  it('setNavItemHidden 追加隐藏项并保存', async () => {
    getPreferencesMock.mockResolvedValue(
      preferences({ hidden_items: ['billingManagement'] })
    )
    const store = useNavigationStore()
    await store.fetchPreferences()

    setHiddenItemsMock.mockResolvedValue(
      preferences({
        hidden_items: ['billingManagement', 'operations'],
      })
    )
    await expect(store.setNavItemHidden('operations', true)).resolves.toBe(true)

    expect(setHiddenItemsMock).toHaveBeenCalledWith(['billingManagement', 'operations'])
    expect(store.hiddenItems).toEqual(['billingManagement', 'operations'])
    expect(store.isItemHidden('operations')).toBe(true)
  })

  it('setNavItemHidden 取消隐藏且不产生重复项', async () => {
    getPreferencesMock.mockResolvedValue(
      preferences({ hidden_items: ['billingManagement', 'operations'] })
    )
    const store = useNavigationStore()
    await store.fetchPreferences()

    setHiddenItemsMock.mockResolvedValue(
      preferences({ hidden_items: ['operations'] })
    )
    await store.setNavItemHidden('billingManagement', false)

    expect(setHiddenItemsMock).toHaveBeenCalledWith(['operations'])
    expect(store.hiddenItems).toEqual(['operations'])
    expect(store.isItemHidden('billingManagement')).toBe(false)
  })

  it('setNavItemHidden 失败时抛出错误且本地状态不变', async () => {
    getPreferencesMock.mockResolvedValue(
      preferences({ hidden_items: ['billingManagement'] })
    )
    const store = useNavigationStore()
    await store.fetchPreferences()

    setHiddenItemsMock.mockRejectedValue(new Error('status 400'))
    await expect(store.setNavItemHidden('operations', true)).rejects.toThrow('status 400')

    expect(errorMock).toHaveBeenCalled()
    expect(store.error).toBe('status 400')
    expect(store.hiddenItems).toEqual(['billingManagement'])
    expect(store.isItemHidden('operations')).toBe(false)
  })
})
