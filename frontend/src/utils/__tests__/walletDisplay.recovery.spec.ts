import { describe, expect, it } from 'vitest'
import { formatWalletCostUnits, rechargeRecoveryStateLabel } from '../walletDisplay'

describe('recovery amount presentation', () => {
  it('retains small debt and safe large integer precision without displaying a false zero', () => {
    expect(formatWalletCostUnits(1)).toBe('$0.00000001')
    expect(formatWalletCostUnits(7_000_000)).toBe('$0.07')
    expect(formatWalletCostUnits(100_000_000)).toBe('$1.00')
    expect(formatWalletCostUnits(2 ** 52)).toBe('$45035996.27370496')
    expect(formatWalletCostUnits(0)).toBe('$0.00')
  })

  it('does not present unavailable or unsafe amounts as zero', () => {
    for (const value of [NaN, Infinity, -1, 0.5, Number.MAX_SAFE_INTEGER + 1]) {
      expect(formatWalletCostUnits(value)).toBe('-')
    }
    expect(rechargeRecoveryStateLabel('future_state')).toBe('状态待确认')
  })
})
