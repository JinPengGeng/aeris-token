import { getI18nLocale } from '@/i18n'
import { parseMoneyUnits } from '@/utils/money'

const COMPACT_NUMBER_UNITS = [
  { value: 1_000_000_000_000, suffix: 'T' },
  { value: 1_000_000_000, suffix: 'B' },
  { value: 1_000_000, suffix: 'M' },
  { value: 1_000, suffix: 'K' },
] as const

interface CompactNumberOptions {
  fractionDigits?: number
  nullLabel?: string
}

function trimTrailingDecimalZeros(value: string): string {
  return value.replace(/\.0+$/, '').replace(/(\.\d*?)0+$/, '$1')
}

function compactFractionDigits(scaled: number, fixedFractionDigits?: number): number {
  if (fixedFractionDigits !== undefined) return fixedFractionDigits
  if (scaled >= 100) return 0
  if (scaled >= 10) return 1
  return 2
}

function formatCompactScaledValue(
  absValue: number,
  unitIndex: number,
  fixedFractionDigits?: number,
): string {
  const unit = COMPACT_NUMBER_UNITS[unitIndex]
  const scaled = absValue / unit.value
  const fractionDigits = compactFractionDigits(scaled, fixedFractionDigits)
  const rounded = Number(scaled.toFixed(fractionDigits))

  if (rounded >= 1000 && unitIndex > 0) {
    return formatCompactScaledValue(absValue, unitIndex - 1, fixedFractionDigits)
  }

  return `${trimTrailingDecimalZeros(scaled.toFixed(fractionDigits))}${unit.suffix}`
}

export function formatCompactNumber(
  num: number | undefined | null,
  options: CompactNumberOptions = {},
): string {
  if (num === undefined || num === null) {
    return options.nullLabel ?? '0'
  }

  const value = Number(num)
  if (!Number.isFinite(value)) {
    return options.nullLabel ?? '0'
  }

  const sign = value < 0 ? '-' : ''
  const absValue = Math.abs(value)

  if (absValue < 1_000) {
    return `${sign}${Number.isInteger(absValue) ? absValue.toString() : trimTrailingDecimalZeros(absValue.toFixed(1))}`
  }

  const unitIndex = COMPACT_NUMBER_UNITS.findIndex(unit => absValue >= unit.value)
  if (unitIndex === -1) {
    return `${sign}${Math.round(absValue)}`
  }

  return `${sign}${formatCompactScaledValue(absValue, unitIndex, options.fractionDigits)}`
}

export function formatByteSize(bytes: number | undefined | null): string {
  if (bytes === undefined || bytes === null || !Number.isFinite(bytes)) {
    return '-'
  }

  const absBytes = Math.max(0, Math.abs(bytes))
  const units = [
    { value: 1024 ** 3, suffix: 'GB' },
    { value: 1024 ** 2, suffix: 'MB' },
    { value: 1024, suffix: 'KB' },
  ] as const
  const unit = units.find(candidate => absBytes >= candidate.value) ?? units[2]
  const scaled = absBytes / unit.value
  const fractionDigits = scaled >= 100 ? 0 : scaled >= 10 ? 1 : 2
  const formatted = trimTrailingDecimalZeros(scaled.toFixed(fractionDigits))

  return `${bytes < 0 ? '-' : ''}${formatted} ${unit.suffix}`
}

// Token formatting - intelligent display based on value size
export function formatTokens(num: number | undefined | null): string {
  return formatCompactNumber(num)
}

// Currency formatting with high precision for small values.
// Accepts fixed-point decimal strings (API money fields) or numbers.
export function formatCurrency(amount: number | string | undefined | null): string {
  if (amount === undefined || amount === null || amount === 0 || amount === '0.00000000') {
    return '$0.00'
  }
  const units = parseMoneyUnits(amount)
  if (units === null) return '$0.00'
  const abs = Math.abs(units)
  const absValue = abs / 1e8

  const formatWith = (decimals: number): string => {
    const factor = 10 ** (8 - decimals)
    const rounded = Math.round(abs / factor)
    const intPart = Math.floor(rounded / 10 ** decimals)
    const fracPart = String(rounded % 10 ** decimals)
      .padStart(decimals, '0')
      .replace(/(\d\d)0+$/, '$1')
    const sign = units < 0 ? '-' : ''
    return `$${sign}${intPart}.${fracPart}`
  }

  if (absValue > 0 && absValue < 0.00001) return formatWith(8)
  if (absValue < 0.0001) return formatWith(6)
  if (absValue < 0.01) return formatWith(5)
  if (absValue < 1) return formatWith(4)
  if (absValue < 100) return formatWith(3)
  return formatWith(2)
}

// Number formatting with locale support
export function formatNumber(num: number | undefined | null): string {
  if (num === undefined || num === null) {
    return '0'
  }
  return num.toLocaleString(getI18nLocale())
}

// Date formatting
export function formatDate(dateString: string | undefined | null): string {
  if (!dateString) return getI18nLocale() === 'en-US' ? 'Unknown' : '未知'

  return new Date(dateString).toLocaleDateString(getI18nLocale(), {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit'
  })
}

export function formatRelativeTime(value: number, unit: Intl.RelativeTimeFormatUnit): string {
  return new Intl.RelativeTimeFormat(getI18nLocale(), { numeric: 'auto' }).format(value, unit)
}

// Model price formatting (already in per 1M tokens)
export function formatModelPrice(price: number | undefined | null): string {
  if (price === undefined || price === null) {
    return '$0.00'
  }

  // Price is already per 1M tokens, no conversion needed
  if (price < 1) {
    return `$${  price.toFixed(4).replace(/\.?0+$/, '').padEnd(price.toFixed(4).indexOf('.') + 3, '0')}`
  } else {
    return `$${  price.toFixed(2)}`
  }
}

// Billing type formatting
export function formatBillingType(type: string | undefined | null): string {
  if (getI18nLocale() === 'en-US') {
    const englishTypeMap: Record<string, string> = {
      'pay_as_you_go': 'Pay as you go',
      'monthly_quota': 'Monthly quota',
      'free_tier': 'Free tier',
    }
    return englishTypeMap[type || ''] || type || 'Pay as you go'
  }

  const typeMap: Record<string, string> = {
    'pay_as_you_go': '按量付费',
    'monthly_quota': '月卡配额',
    'free_tier': '免费套餐'
  }
  return typeMap[type || ''] || type || '按量付费'
}

// Format cost with 4 decimal places (for cache analysis)
export function formatCost(cost: number | null | undefined): string {
  if (cost === null || cost === undefined) return '-'
  return `$${cost.toFixed(4)}`
}

// Usage count formatting (compact display for large numbers)
export function formatUsageCount(count: number): string {
  return formatCompactNumber(count, { fractionDigits: 1 })
}

// Format remaining time from unix timestamp
export function formatRemainingTime(expireAt: number | undefined, currentTime: number): string {
  const isEnglish = getI18nLocale() === 'en-US'
  if (!expireAt) return isEnglish ? 'Unknown' : '未知'
  const remaining = expireAt - currentTime
  if (remaining <= 0) return isEnglish ? 'Expired' : '已过期'

  const minutes = Math.floor(remaining / 60)
  const seconds = Math.floor(remaining % 60)
  return isEnglish ? `${minutes}m ${seconds}s` : `${minutes}分${seconds}秒`
}

// Cache hit rate formatting
export function formatHitRate(rate: number | undefined): string {
  if (typeof rate !== 'number' || Number.isNaN(rate)) return '-'
  return `${rate.toFixed(2)}%`
}

// Rate limit formatting (supports "inherit" semantics: null = inherit system default)
export function formatRateLimitInheritable(rateLimit?: number | null): string {
  if (rateLimit == null) return getI18nLocale() === 'en-US' ? 'Use system default' : '跟随系统'
  if (rateLimit === 0) return getI18nLocale() === 'en-US' ? 'No limit' : '不限速'
  return `${rateLimit}/min`
}

// Rate limit formatting (simple: null/0 both mean unlimited)
export function formatRateLimitSimple(rateLimit?: number | null): string {
  if (rateLimit == null || rateLimit === 0) return getI18nLocale() === 'en-US' ? 'No limit' : '不限速'
  return `${rateLimit}/min`
}

// Rate limit state helpers
export function isRateLimitInherited(rateLimit?: number | null): boolean {
  return rateLimit == null
}

export function isRateLimitUnlimited(rateLimit?: number | null): boolean {
  return rateLimit === 0
}

export function formatShortRequestId(value: string | null | undefined): string {
  const trimmed = value?.trim()
  if (!trimmed) return '-'
  if (trimmed.length <= 12) return trimmed

  const uuidLike = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(trimmed)
  if (uuidLike) {
    return trimmed.slice(0, 8)
  }

  return `${trimmed.slice(0, 6)}...${trimmed.slice(-4)}`
}
