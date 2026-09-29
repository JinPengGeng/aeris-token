import { defineStore } from 'pinia'
import { ref } from 'vue'
import {
  navigationApi,
  type NavigationItemDefinition,
  type NavigationPreferences,
} from '@/api/navigation'
import { log } from '@/utils/logger'
import { parseApiError } from '@/utils/errorParser'

/**
 * 管理端内置导航项可见性配置（issue #573）。
 *
 * 隐藏仅作用于侧边栏菜单与面包屑展示；路由与权限不变，深链接仍可访问。
 * 管理端“菜单管理”分区（/admin/modules）负责编辑该配置。
 */
export const useNavigationStore = defineStore('navigation', () => {
  const hiddenItems = ref<string[]>([])
  const items = ref<NavigationItemDefinition[]>([])
  const loaded = ref(false)
  const loading = ref(false)
  const error = ref<string | null>(null)
  let fetchPreferencesPromise: Promise<NavigationPreferences> | null = null

  /**
   * 获取导航项可见性配置（结果去重，进行中的请求直接复用）
   */
  async function fetchPreferences(): Promise<NavigationPreferences> {
    if (fetchPreferencesPromise) return fetchPreferencesPromise

    loading.value = true
    error.value = null

    fetchPreferencesPromise = (async () => {
      try {
        const preferences = await navigationApi.getPreferences()
        hiddenItems.value = preferences.hidden_items
        items.value = preferences.items
        loaded.value = true
        return preferences
      } catch (err: unknown) {
        log.error('Failed to fetch navigation preferences', err)
        error.value = parseApiError(err, '获取菜单显示配置失败')
        throw err
      } finally {
        loading.value = false
        fetchPreferencesPromise = null
      }
    })()

    return fetchPreferencesPromise
  }

  /**
   * 判断内置管理导航项是否被隐藏
   */
  function isItemHidden(key: string): boolean {
    return hiddenItems.value.includes(key)
  }

  /**
   * 设置内置管理导航项是否隐藏
   * @throws 如果保存失败会抛出错误（状态保持不变，由调用方提示）
   */
  async function setNavItemHidden(key: string, hidden: boolean) {
    const nextHiddenItems = hidden
      ? [...new Set([...hiddenItems.value, key])]
      : hiddenItems.value.filter(item => item !== key)
    try {
      const preferences = await navigationApi.setHiddenItems(nextHiddenItems)
      hiddenItems.value = preferences.hidden_items
      items.value = preferences.items
      return true
    } catch (err: unknown) {
      log.error(`Failed to set navigation item ${key} hidden=${hidden}`, err)
      error.value = parseApiError(err, '保存菜单显示配置失败')
      // 重新抛出错误，让调用方可以获取详细错误信息
      throw err
    }
  }

  return {
    hiddenItems,
    items,
    loaded,
    loading,
    error,
    fetchPreferences,
    isItemHidden,
    setNavItemHidden,
  }
})
