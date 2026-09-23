// 金额定点工具:后端 API 的金额字段统一为 8 位小数字符串(1e-8 USD 微单位)。
// 前端仅在展示边界转换为 number,展示截断用字符串操作避免浮点误差。

export const MONEY_DECIMALS = 8

// 字符串定点 → 微单位整数;非法输入返回 null(不静默当 0,由调用方决定降级策略)
export function parseMoneyUnits(value: string | number | null | undefined): number | null {
  if (value === null || value === undefined) return null
  const text = String(value).trim()
  if (!/^-?\d+(\.\d{1,8})?$/.test(text)) return null
  const negative = text.startsWith('-')
  const body = negative ? text.slice(1) : text
  const [intPart, fracPart = ''] = body.split('.')
  const units = Number(intPart) * 1e8 + Number(fracPart.padEnd(8, '0'))
  if (!Number.isSafeInteger(units)) return null
  return negative ? -units : units
}

// 金额(字符串/数字)安全转 number;无法解析时返回 fallback
export function moneyToNumber(
  value: string | number | null | undefined,
  fallback = 0
): number {
  return parseMoneyUnits(value) ?? fallback
}

// 金额展示:基于微单位做截断/四舍五入,避免二进制浮点误差(如 0.1+0.2)
export function formatMoney(
  value: string | number | null | undefined,
  decimals = 2,
  options?: { trim?: boolean }
): string {
  const units = parseMoneyUnits(value)
  if (units === null) return Number(value ?? 0).toFixed(decimals)
  const negative = units < 0
  const abs = Math.abs(units)
  const factor = 10 ** (MONEY_DECIMALS - Math.min(decimals, MONEY_DECIMALS))
  const rounded = Math.round(abs / factor)
  const intPart = Math.floor(rounded / 10 ** decimals)
  const fracPart = String(rounded % 10 ** decimals).padStart(decimals, '0')
  let frac = fracPart
  if (options?.trim) {
    frac = frac.replace(/0+$/, '').replace(/\.$/, '')
  }
  return `${negative ? '-' : ''}${intPart}${frac ? `.${frac}` : decimals > 0 && !options?.trim ? `.${fracPart}` : ''}`
}

// 表单提交:把用户输入的金额格式化为 8 位小数字符串定点
export function toMoneyString(value: string | number | null | undefined): string {
  const units = parseMoneyUnits(value)
  if (units === null) return '0.00000000'
  const negative = units < 0
  const abs = Math.abs(units)
  const intPart = Math.floor(abs / 1e8)
  const fracPart = String(abs % 1e8).padStart(8, '0')
  return `${negative ? '-' : ''}${intPart}.${fracPart}`
}

export function isPositiveMoney(value: string | number | null | undefined): boolean {
  const units = parseMoneyUnits(value)
  return units !== null && units > 0
}

export function compareMoney(
  a: string | number | null | undefined,
  b: string | number | null | undefined
): number {
  return (parseMoneyUnits(a) ?? 0) - (parseMoneyUnits(b) ?? 0)
}
