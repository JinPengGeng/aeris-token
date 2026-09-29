import apiClient from './client'

/**
 * 可隐藏的内置管理导航项定义（后端 GET /api/admin/navigation/preferences
 * 返回的 items 元素）。
 */
export interface NavigationItemDefinition {
  key: string
  href: string
  menu_group: 'overview' | 'management' | 'system'
  display_name: string
}

export interface NavigationPreferences {
  hidden_items: string[]
  items: NavigationItemDefinition[]
}

const NAVIGATION_MENU_GROUPS = ['overview', 'management', 'system'] as const

/**
 * 归一化隐藏清单：仅保留字符串、去空白、去重、保持顺序。
 * 与后端存储侧归一化语义一致，防御历史脏数据。
 */
export function normalizeHiddenNavItems(value: unknown): string[] {
  if (!Array.isArray(value)) return []
  const seen = new Set<string>()
  const items: string[] = []
  for (const entry of value) {
    if (typeof entry !== 'string') continue
    const key = entry.trim()
    if (!key || seen.has(key)) continue
    seen.add(key)
    items.push(key)
  }
  return items
}

function normalizeNavigationItem(value: unknown): NavigationItemDefinition | null {
  if (!value || typeof value !== 'object') return null
  const item = value as Record<string, unknown>
  const key = typeof item.key === 'string' ? item.key : ''
  const href = typeof item.href === 'string' ? item.href : ''
  if (!key || !href) return null
  const menuGroup = NAVIGATION_MENU_GROUPS.find(group => group === item.menu_group)
  return {
    key,
    href,
    menu_group: menuGroup ?? 'management',
    display_name: typeof item.display_name === 'string' ? item.display_name : key,
  }
}

function normalizeNavigationPreferences(payload: unknown): NavigationPreferences {
  const raw = (payload ?? {}) as Record<string, unknown>
  const items = Array.isArray(raw.items)
    ? raw.items
        .map(normalizeNavigationItem)
        .filter((item): item is NavigationItemDefinition => item !== null)
    : []
  return {
    hidden_items: normalizeHiddenNavItems(raw.hidden_items),
    items,
  }
}

export const navigationApi = {
  /**
   * 获取管理端内置导航项可见性配置（管理员）。
   * hidden_items 为隐藏 key 清单；未配置时为空数组（全部显示）。
   */
  async getPreferences(): Promise<NavigationPreferences> {
    const response = await apiClient.get<NavigationPreferences>(
      '/api/admin/navigation/preferences'
    )
    return normalizeNavigationPreferences(response.data)
  },

  /**
   * 全量覆盖隐藏清单；传空数组恢复全部内置项显示。
   */
  async setHiddenItems(hiddenItems: string[]): Promise<NavigationPreferences> {
    const response = await apiClient.put<NavigationPreferences>(
      '/api/admin/navigation/preferences',
      { hidden_items: normalizeHiddenNavItems(hiddenItems) }
    )
    return normalizeNavigationPreferences(response.data)
  },
}
