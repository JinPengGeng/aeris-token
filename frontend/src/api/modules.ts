import apiClient from './client'
import { buildCacheKey, cache, cachedRequest } from '@/utils/cache'
import { useAuthStore } from '@/stores/auth'

const MODULE_MANAGEMENT_ORDER_CONFIG_KEY = 'module_management.extension_order'
const ALL_SYSTEM_CONFIGS_CACHE_KEY = 'admin:system:configs'

export interface ModuleStatus {
  name: string
  available: boolean
  enabled: boolean
  active: boolean
  config_validated: boolean
  config_error: string | null
  display_name: string
  description: string
  category: 'auth' | 'monitoring' | 'security' | 'integration'
  admin_route: string | null
  admin_menu_icon: string | null
  admin_menu_group: string | null
  admin_menu_order: number
  health: 'healthy' | 'degraded' | 'unhealthy' | 'unknown'
}

export interface AuthModuleInfo {
  name: string
  display_name: string
  active: boolean
}

export type ChatPiiRedactionTtlSeconds = 300 | 3600

export interface ChatPiiRedactionRuleFeatures {
  validator?: string | null
  [key: string]: unknown
}

export interface ChatPiiRedactionRule {
  id: string
  name: string
  pattern: string
  enabled: boolean
  system?: boolean
  features?: ChatPiiRedactionRuleFeatures | null
}

export interface ChatPiiRedactionConfig {
  enabled: boolean
  rules: ChatPiiRedactionRule[]
  cache_ttl_seconds: ChatPiiRedactionTtlSeconds
  placeholder_prefix: string
}

export const CHAT_PII_REDACTION_DEFAULT_RULES: ChatPiiRedactionRule[] = [
  { id: 'email', name: '邮箱', pattern: '(?i)[A-Z0-9._%+-]{1,64}@[A-Z0-9.-]{1,253}\\.[A-Z]{2,63}', enabled: true, features: { validator: 'email' }, system: true },
  { id: 'cn_phone', name: '手机号', pattern: '(?:\\+?86[- ]?)?(?:1[3-9]\\d[- ]?\\d{4}[- ]?\\d{4}|0\\d{2,3}[- ]\\d{7,8}(?:-\\d{1,6})?)', enabled: true, features: { validator: 'cn_phone' }, system: true },
  { id: 'global_phone', name: '国际号码', pattern: '\\+[1-9]\\d(?:[ -]?\\d){6,13}\\d', enabled: true, features: { validator: 'global_phone' }, system: true },
  { id: 'cn_id', name: '身份证号', pattern: '(?i)\\b\\d{17}[\\dX]\\b', enabled: true, features: { validator: 'cn_id' }, system: true },
  { id: 'payment_card', name: '银行卡号', pattern: '\\b(?:\\d[ -]?){12,18}\\d\\b', enabled: true, features: { validator: 'payment_card' }, system: true },
  { id: 'ipv4', name: 'IPv4', pattern: '\\b(?:\\d{1,3}\\.){3}\\d{1,3}\\b', enabled: true, features: { validator: 'ipv4' }, system: true },
  { id: 'ipv6', name: 'IPv6', pattern: '\\b(?:[0-9A-Fa-f]{1,4}:){2,7}[0-9A-Fa-f:.]{1,39}\\b', enabled: true, features: { validator: 'ipv6' }, system: true },
  { id: 'api_key', name: 'API Key', pattern: '\\b(?:sk-(?:proj-)?[A-Za-z0-9_-]{20,}|sk-ant-[A-Za-z0-9_-]{20,}|(?:gh[pousr]_[A-Za-z0-9_]{30,}|github_pat_[A-Za-z0-9_]{30,})|xox[baprs]-[A-Za-z0-9-]{20,}|(?:AKIA|ASIA)[0-9A-Z]{16}|[A-Za-z0-9_-]{32,})\\b', enabled: true, features: { validator: 'api_key' }, system: true },
  { id: 'access_token', name: 'Access Token', pattern: "(?i)\\baccess[_-]?token\\s*[:=]\\s*[\"']?[A-Za-z0-9._~+/=-]{20,}", enabled: true, features: { validator: 'access_token' }, system: true },
  { id: 'secret_key', name: 'Secret Key', pattern: "(?i)\\bsecret[_-]?key\\s*[:=]\\s*[\"']?[A-Za-z0-9._~+/=-]{20,}", enabled: true, features: { validator: 'secret_key' }, system: true },
  { id: 'bearer_token', name: 'Bearer Token', pattern: '(?i)\\bBearer\\s+[A-Za-z0-9._~+/=-]{20,}', enabled: true, features: { validator: 'bearer_token' }, system: true },
  { id: 'jwt', name: 'JWT', pattern: '\\b[A-Za-z0-9_-]{10,}\\.[A-Za-z0-9_-]{10,}\\.[A-Za-z0-9_-]{10,}\\b', enabled: true, features: { validator: 'jwt' }, system: true },
]

const CHAT_PII_REDACTION_CONFIG_KEYS = {
  enabled: 'module.chat_pii_redaction.enabled',
  rules: 'module.chat_pii_redaction.rules',
  cache_ttl_seconds: 'module.chat_pii_redaction.cache_ttl_seconds',
  placeholder_prefix: 'module.chat_pii_redaction.placeholder_prefix',
} as const

const CHAT_PII_REDACTION_DEFAULT_CONFIG: ChatPiiRedactionConfig = {
  enabled: false,
  rules: CHAT_PII_REDACTION_DEFAULT_RULES.map(rule => ({ ...rule })),
  cache_ttl_seconds: 300,
  placeholder_prefix: 'AETHER',
}

export function normalizeModuleManagementOrder(value: unknown): string[] {
  if (!Array.isArray(value)) return []
  const seen = new Set<string>()
  const order: string[] = []
  for (const item of value) {
    if (typeof item !== 'string') continue
    const name = item.trim()
    if (!name || seen.has(name)) continue
    seen.add(name)
    order.push(name)
  }
  return order
}

function cloneDefaultChatPiiRedactionRules(): ChatPiiRedactionRule[] {
  return CHAT_PII_REDACTION_DEFAULT_RULES.map(rule => ({ ...rule }))
}

function normalizeChatPiiRedactionRule(value: unknown, index: number): ChatPiiRedactionRule | null {
  if (!value || typeof value !== 'object') return null
  const item = value as Record<string, unknown>
  const id = typeof item.id === 'string' && item.id.trim()
    ? item.id.trim()
    : `custom_${index + 1}`
  const name = typeof item.name === 'string' && item.name.trim()
    ? item.name.trim()
    : id
  const pattern = typeof item.pattern === 'string' ? item.pattern : ''
  if (!pattern.trim()) return null
  const rawFeatures = item.features && typeof item.features === 'object' && !Array.isArray(item.features)
    ? { ...(item.features as Record<string, unknown>) }
    : {}
  const legacyValidator = typeof item.kind === 'string' && item.kind.trim()
    ? item.kind.trim()
    : null
  const validator = typeof rawFeatures.validator === 'string' && rawFeatures.validator.trim()
    ? rawFeatures.validator.trim()
    : legacyValidator
  if (validator) {
    rawFeatures.validator = validator
  } else {
    delete rawFeatures.validator
  }
  const features = Object.keys(rawFeatures).length > 0 ? rawFeatures : null
  return {
    id,
    name,
    pattern,
    enabled: item.enabled !== false,
    system: item.system === true,
    features,
  }
}

function normalizeChatPiiRedactionRules(value: unknown): ChatPiiRedactionRule[] {
  if (!Array.isArray(value)) return cloneDefaultChatPiiRedactionRules()
  return value
    .map((item, index) => normalizeChatPiiRedactionRule(item, index))
    .filter((item): item is ChatPiiRedactionRule => item !== null)
}

function normalizeChatPiiRedactionConfig(values: {
  enabled: unknown
  rules: unknown
  cache_ttl_seconds: unknown
  placeholder_prefix: unknown
}): ChatPiiRedactionConfig {
  return {
    enabled: values.enabled === true,
    rules: normalizeChatPiiRedactionRules(values.rules),
    cache_ttl_seconds: values.cache_ttl_seconds === 3600 ? 3600 : 300,
    placeholder_prefix: normalizePlaceholderPrefix(values.placeholder_prefix),
  }
}

function normalizePlaceholderPrefix(value: unknown): string {
  if (typeof value !== 'string') return CHAT_PII_REDACTION_DEFAULT_CONFIG.placeholder_prefix
  const normalized = value.trim().toUpperCase()
  return /^[A-Z0-9_]{1,32}$/.test(normalized)
    ? normalized
    : CHAT_PII_REDACTION_DEFAULT_CONFIG.placeholder_prefix
}

async function updateSystemConfigValue(key: string, value: unknown, description: string) {
  const response = await apiClient.put<{ key: string; value: unknown; description?: string }>(
    `/api/admin/system/configs/${key}`,
    { value, description },
  )
  cache.delete(ALL_SYSTEM_CONFIGS_CACHE_KEY)
  cache.delete(buildCacheKey('admin:system:config', { key }))
  return response.data.value
}

async function getAllSystemConfigValues(): Promise<Map<string, unknown>> {
  const configs = await cachedRequest(
    ALL_SYSTEM_CONFIGS_CACHE_KEY,
    async () => {
      const response = await apiClient.get<Array<{ key: string; value: unknown }>>(
        '/api/admin/system/configs'
      )
      return response.data
    },
    30_000,
  )
  return new Map(configs.map(config => [config.key, config.value]))
}

/**
 * 用户侧只读模块状态（GET /api/modules/status 的返回项）。
 * 仅暴露 name 与 active，不含管理端内部字段。
 */
export interface UserModuleStatus {
  name: string
  active: boolean
}

/**
 * 将用户侧只读 payload 归一化为 ModuleStatus 形状。
 *
 * 后端语义：active = available && enabled && config_validated，因此
 * active=true 蕴含 available/enabled/config_validated=true；active=false
 * 时三者一律取 false（fail-close）。管理端专用字段（admin_route 等）
 * 对普通用户不可见也不可用，保持中性默认值。
 */
function normalizeUserModuleStatuses(
  payload: Record<string, UserModuleStatus>
): Record<string, ModuleStatus> {
  const normalized: Record<string, ModuleStatus> = {}
  for (const [name, status] of Object.entries(payload)) {
    const active = status?.active === true
    normalized[name] = {
      name,
      available: active,
      enabled: active,
      active,
      config_validated: active,
      config_error: null,
      display_name: name,
      description: '',
      category: 'integration',
      admin_route: null,
      admin_menu_icon: null,
      admin_menu_group: null,
      admin_menu_order: 0,
      health: 'unknown',
    }
  }
  return normalized
}

export const modulesApi = {
  /**
   * 获取所有模块状态。
   *
   * 管理员走 /api/admin/modules/status（完整字段，供管理端菜单/守卫使用）；
   * 普通用户走用户侧只读端点 /api/modules/status（仅 name + active），
   * 避免普通用户请求管理端接口 403 导致模块状态永远加载失败（issue #579）。
   */
  async getAllStatus(): Promise<Record<string, ModuleStatus>> {
    // 仅在调用时读取角色（auth store 已随应用激活），api 层不缓存角色状态。
    if (useAuthStore().canAccessAdmin) {
      const response = await apiClient.get<Record<string, ModuleStatus>>(
        '/api/admin/modules/status'
      )
      return response.data
    }
    const response = await apiClient.get<Record<string, UserModuleStatus>>(
      '/api/modules/status'
    )
    return normalizeUserModuleStatuses(response.data)
  },

  /**
   * 获取单个模块状态（管理员）
   */
  async getStatus(moduleName: string): Promise<ModuleStatus> {
    const response = await apiClient.get<ModuleStatus>(
      `/api/admin/modules/status/${moduleName}`
    )
    return response.data
  },

  /**
   * 设置模块启用状态（管理员）
   */
  async setEnabled(moduleName: string, enabled: boolean): Promise<ModuleStatus> {
    const response = await apiClient.put<ModuleStatus>(
      `/api/admin/modules/status/${moduleName}/enabled`,
      { enabled }
    )
    cache.delete(ALL_SYSTEM_CONFIGS_CACHE_KEY)
    return response.data
  },

  async getModuleManagementOrder(): Promise<string[]> {
    try {
      const response = await apiClient.get<{ key: string; value: unknown }>(
        `/api/admin/system/configs/${MODULE_MANAGEMENT_ORDER_CONFIG_KEY}`
      )
      return normalizeModuleManagementOrder(response.data.value)
    } catch (err) {
      const status = (err as { response?: { status?: number } }).response?.status
      if (status === 404) return []
      throw err
    }
  },

  async updateModuleManagementOrder(order: string[]): Promise<string[]> {
    const normalized = normalizeModuleManagementOrder(order)
    const response = await apiClient.put<{ key: string; value: unknown }>(
      `/api/admin/system/configs/${MODULE_MANAGEMENT_ORDER_CONFIG_KEY}`,
      {
        value: normalized,
        description: '模块管理扩展模块展示顺序',
      },
    )
    cache.delete(ALL_SYSTEM_CONFIGS_CACHE_KEY)
    cache.delete(buildCacheKey('admin:system:config', {
      key: MODULE_MANAGEMENT_ORDER_CONFIG_KEY,
    }))
    return normalizeModuleManagementOrder(response.data.value)
  },

  async getChatPiiRedactionConfig(): Promise<ChatPiiRedactionConfig> {
    const configsByKey = await getAllSystemConfigValues()

    return normalizeChatPiiRedactionConfig({
      enabled: configsByKey.get(CHAT_PII_REDACTION_CONFIG_KEYS.enabled),
      rules: configsByKey.get(CHAT_PII_REDACTION_CONFIG_KEYS.rules),
      cache_ttl_seconds: configsByKey.get(CHAT_PII_REDACTION_CONFIG_KEYS.cache_ttl_seconds),
      placeholder_prefix: configsByKey.get(CHAT_PII_REDACTION_CONFIG_KEYS.placeholder_prefix),
    })
  },

  async updateChatPiiRedactionConfig(config: ChatPiiRedactionConfig): Promise<ChatPiiRedactionConfig> {
    const [enabled, rules, cacheTtlSeconds, placeholderPrefix] = await Promise.all([
      updateSystemConfigValue(CHAT_PII_REDACTION_CONFIG_KEYS.enabled, config.enabled, '敏感信息保护总开关'),
      updateSystemConfigValue(CHAT_PII_REDACTION_CONFIG_KEYS.rules, config.rules, '敏感信息保护替换规则'),
      updateSystemConfigValue(CHAT_PII_REDACTION_CONFIG_KEYS.cache_ttl_seconds, config.cache_ttl_seconds, '敏感信息保护缓存 TTL'),
      updateSystemConfigValue(CHAT_PII_REDACTION_CONFIG_KEYS.placeholder_prefix, config.placeholder_prefix, '敏感信息保护占位符前缀'),
    ])

    return normalizeChatPiiRedactionConfig({
      enabled,
      rules,
      cache_ttl_seconds: cacheTtlSeconds,
      placeholder_prefix: placeholderPrefix,
    })
  },

  /**
   * 获取认证模块状态（公开接口，供登录页使用）
   */
  async getAuthModulesStatus(): Promise<AuthModuleInfo[]> {
    const response = await apiClient.get<AuthModuleInfo[]>('/api/modules/auth-status')
    return response.data
  },
}
