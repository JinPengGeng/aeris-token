import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, nextTick, type App } from 'vue'
import type { PaymentOrder, WalletBalanceResponse, WalletRechargeRecovery } from '@/api/wallet'
import WalletCenter from '../WalletCenter.vue'

const walletApiMock = vi.hoisted(() => ({
  getBalance: vi.fn(),
  getFlow: vi.fn(),
  getTodayCost: vi.fn(),
  listRechargeOptions: vi.fn(),
  listRechargeOrders: vi.fn(),
  getRechargeOrder: vi.fn(),
  createRechargeOrder: vi.fn(),
  listRechargeRecoveries: vi.fn(),
}))
const toastMock = vi.hoisted(() => ({ success: vi.fn(), info: vi.fn(), error: vi.fn() }))
const billingApiMock = vi.hoisted(() => ({ listEntitlements: vi.fn() }))

vi.mock('@/api/wallet', () => ({ walletApi: walletApiMock }))
vi.mock('@/api/billing', () => ({ billingApi: billingApiMock }))
vi.mock('@/composables/useToast', () => ({ useToast: () => toastMock }))
vi.mock('@/utils/logger', () => ({ log: { error: vi.fn() } }))
vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')
  const passthrough = defineComponent({ setup: (_, { slots }) => () => h('div', slots.default?.()) })
  const button = defineComponent({ setup: (_, { slots }) => () => h('button', slots.default?.()) })
  return {
    ...Object.fromEntries([
      'Badge', 'Card', 'Input', 'Label', 'Select', 'SelectContent', 'SelectItem',
      'SelectTrigger', 'SelectValue', 'Table', 'TableBody', 'TableCell', 'TableHead',
      'TableHeader', 'TableRow', 'Tabs', 'TabsContent', 'TabsList', 'TabsTrigger', 'Textarea',
    ].map(name => [name, passthrough])),
    Button: button,
    RefreshButton: defineComponent({ setup: () => () => h('button', { 'data-refresh': true }, '刷新') }),
    Pagination: defineComponent({
      props: { current: Number },
      emits: ['update:current'],
      setup: (_, { emit }) => () => h('button', {
        'data-next-page': true,
        onClick: () => emit('update:current', 2),
      }, '下一页'),
    }),
  }
})
vi.mock('@/components/common', async () => {
  const { defineComponent, h } = await import('vue')
  const empty = defineComponent({ setup: () => () => h('div') })
  return {
    EmptyState: empty,
    LoadingState: empty,
    StripePaymentDialog: defineComponent({
      emits: ['success'],
      setup: (_, { emit }) => () => h('button', {
        'data-stripe-success': true,
        onClick: () => emit('success', { intentId: 'pi-1', status: 'processing' }),
      }, 'Stripe 提交'),
    }),
  }
})

const mountedApps: Array<{ app: App; root: HTMLElement }> = []
let hidden = false

function walletBalance(amount: number): WalletBalanceResponse {
  return {
    wallet: {
      id: 'wallet-1', balance: amount, recharge_balance: amount, gift_balance: 0,
      refundable_balance: amount, currency: 'USD', status: 'active', total_recharged: amount,
      total_consumed: 0, total_refunded: 0, total_adjusted: 0, updated_at: '2026-09-11T00:00:00Z',
    },
    balance: amount, unlimited: false, limit_mode: 'finite', currency: 'USD',
    wallet_balance: amount, package_balance: 3, total_available_balance: amount + 3,
    daily_quota: { has_active: true, total_usd: 5, used_usd: 2, remaining_usd: 3, allow_wallet_overage: true },
  }
}

function paymentOrder(status = 'pending', overrides: Partial<PaymentOrder> = {}): PaymentOrder {
  return {
    id: 'order-1', order_no: 'RECHARGE-1', wallet_id: 'wallet-1', user_id: 'user-1',
    amount_usd: 10, pay_amount: 10, pay_currency: 'USD', exchange_rate: 1,
    refunded_amount_usd: 0, refundable_amount_usd: status === 'credited' ? 10 : 0,
    payment_method: 'epay', gateway_order_id: 'gateway-1', gateway_response: null,
    status, created_at: '2026-09-11T00:00:00Z', paid_at: null,
    credited_at: status === 'credited' ? '2026-09-11T00:01:00Z' : null, expires_at: null,
    ...overrides,
  }
}

function orderResponse(items: PaymentOrder[], amount = 2, offset = 0) {
  const { wallet_balance: _wallet, package_balance: _package, total_available_balance: _total, daily_quota: _quota, ...balance } = walletBalance(amount)
  return { ...balance, items, total: items.length, limit: 20, offset }
}

function recovery(overrides: Partial<WalletRechargeRecovery> = {}): WalletRechargeRecovery {
  return {
    id: 'private-job-id', payment_order_id: 'order-1', wallet_id: 'private-wallet-id',
    state: 'pending', principal_cost_units: 1_000_000_000,
    collected_cost_units: 0, outstanding_cost_units: 700_000_000,
    available_recharge_cost_units: 1_000_000_000,
    retry_count: 0, next_attempt_at_unix_secs: 1_800_000_000,
    error_code: null, created_at_unix_secs: 1_799_999_000,
    updated_at_unix_secs: 1_799_999_000, ...overrides,
  }
}

async function flushPromises() {
  for (let i = 0; i < 10; i += 1) await Promise.resolve()
  await nextTick()
}

async function mountWallet() {
  const root = document.createElement('div')
  document.body.append(root)
  const app = createApp(WalletCenter)
  app.mount(root)
  mountedApps.push({ app, root })
  await flushPromises()
  return { app, root }
}

function setHidden(value: boolean) {
  hidden = value
  document.dispatchEvent(new Event('visibilitychange'))
}

beforeEach(() => {
  vi.useFakeTimers()
  vi.resetAllMocks()
  hidden = false
  vi.spyOn(document, 'hidden', 'get').mockImplementation(() => hidden)
  walletApiMock.getBalance.mockResolvedValue(walletBalance(2))
  walletApiMock.getFlow.mockResolvedValue({ items: [], total: 0, today_entry: null })
  walletApiMock.getTodayCost.mockResolvedValue(null)
  walletApiMock.listRechargeOptions.mockResolvedValue({ items: [] })
  walletApiMock.listRechargeOrders.mockResolvedValue(orderResponse([paymentOrder()]))
  walletApiMock.listRechargeRecoveries.mockResolvedValue({ items: [], limit: 50 })
  billingApiMock.listEntitlements.mockResolvedValue({ items: [] })
})

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
  vi.restoreAllMocks()
  vi.useRealTimers()
})

describe('WalletCenter recharge synchronization', () => {
  it('waits for server credit after Stripe submission and refreshes the credited balance and flow', async () => {
    const { root } = await mountWallet()
    root.querySelector<HTMLButtonElement>('[data-stripe-success]')!.click()
    await flushPromises()
    expect(toastMock.info).toHaveBeenCalledWith('支付已提交，正在等待充值到账')
    expect(toastMock.success).not.toHaveBeenCalled()

    walletApiMock.listRechargeOrders.mockResolvedValue(orderResponse([paymentOrder('paid')]))
    await vi.advanceTimersByTimeAsync(5_000)
    expect(toastMock.success).not.toHaveBeenCalled()

    // The order query can observe the credit after its wallet snapshot was read.
    walletApiMock.listRechargeOrders.mockResolvedValue(orderResponse([paymentOrder('credited')], 2))
    walletApiMock.getBalance.mockResolvedValue(walletBalance(12))
    await vi.advanceTimersByTimeAsync(5_000)
    expect(root.textContent).toContain('$12.00')
    expect(root.textContent).toContain('$15.00')
    expect(walletApiMock.getFlow).toHaveBeenCalledTimes(2)
    expect(toastMock.success).toHaveBeenCalledWith('充值已到账，余额已更新')

    const calls = walletApiMock.listRechargeOrders.mock.calls.length
    await vi.advanceTimersByTimeAsync(15_000)
    expect(walletApiMock.listRechargeOrders).toHaveBeenCalledTimes(calls)
  })

  it('updates the wallet and preserves package quota when the orders refresh button is clicked', async () => {
    walletApiMock.listRechargeOrders.mockResolvedValue(orderResponse([paymentOrder('credited')], 2))
    const { root } = await mountWallet()
    walletApiMock.listRechargeOrders.mockResolvedValue(orderResponse([paymentOrder('credited')], 12))

    root.querySelector<HTMLButtonElement>('[value="orders"] [data-refresh]')!.click()
    await flushPromises()

    expect(root.textContent).toContain('$12.00')
    expect(root.textContent).toContain('$15.00')
    expect(root.textContent).toContain('已用 $2.00 / 每日 $5.00')
    expect(walletApiMock.getBalance).toHaveBeenCalledTimes(1)
  })

  it('refreshes entitlements and the wallet after the order snapshot finishes', async () => {
    const { root } = await mountWallet()
    let resolve!: (value: ReturnType<typeof orderResponse>) => void
    walletApiMock.listRechargeOrders.mockReturnValue(new Promise(complete => { resolve = complete }))
    walletApiMock.getBalance.mockResolvedValue(walletBalance(12))

    root.querySelector<HTMLButtonElement>('#wallet-redeem [data-refresh]')!.click()
    await flushPromises()
    expect(walletApiMock.getBalance).toHaveBeenCalledTimes(1)
    expect(billingApiMock.listEntitlements).toHaveBeenCalledTimes(1)

    resolve(orderResponse([paymentOrder()], 2))
    await flushPromises()
    expect(walletApiMock.getBalance).toHaveBeenCalledTimes(2)
    expect(billingApiMock.listEntitlements).toHaveBeenCalledTimes(2)
    expect(walletApiMock.getFlow).toHaveBeenCalledTimes(2)
    expect(root.textContent).toContain('$12.00')
    expect(root.textContent).toContain('$15.00')
  })

  it('pauses polling while hidden and checks credit as soon as the page becomes visible', async () => {
    const { root } = await mountWallet()
    setHidden(true)
    await vi.advanceTimersByTimeAsync(20_000)
    expect(walletApiMock.listRechargeOrders).toHaveBeenCalledTimes(1)

    walletApiMock.listRechargeOrders.mockResolvedValue(orderResponse([paymentOrder('credited')], 12))
    walletApiMock.getBalance.mockResolvedValue(walletBalance(12))
    setHidden(false)
    await flushPromises()

    expect(walletApiMock.listRechargeOrders).toHaveBeenCalledTimes(2)
    expect(root.textContent).toContain('$12.00')
    expect(walletApiMock.getFlow).toHaveBeenCalledTimes(2)
  })

  it('keeps tracking a pending recharge after the user changes the order page', async () => {
    const { root } = await mountWallet()
    walletApiMock.listRechargeOrders.mockResolvedValue(orderResponse([], 2, 20))
    walletApiMock.getRechargeOrder.mockResolvedValue({ order: paymentOrder('credited') })
    walletApiMock.getBalance.mockResolvedValue(walletBalance(12))
    root.querySelector<HTMLButtonElement>('[value="orders"] [data-next-page]')!.click()
    await flushPromises()

    expect(walletApiMock.getRechargeOrder).toHaveBeenCalledWith('order-1')
    expect(root.textContent).toContain('$12.00')
    expect(toastMock.success).toHaveBeenCalledTimes(1)
  })

  it('retries a temporary polling failure without showing an error toast', async () => {
    const { root } = await mountWallet()
    walletApiMock.listRechargeOrders.mockRejectedValueOnce(new Error('network unavailable'))
    await vi.advanceTimersByTimeAsync(5_000)
    expect(toastMock.error).not.toHaveBeenCalled()

    walletApiMock.listRechargeOrders.mockResolvedValue(orderResponse([paymentOrder('credited')], 12))
    walletApiMock.getBalance.mockResolvedValue(walletBalance(12))
    await vi.advanceTimersByTimeAsync(5_000)
    expect(root.textContent).toContain('$12.00')
    expect(toastMock.success).toHaveBeenCalledTimes(1)
  })

  it('does not restart polling when an in-flight response completes after unmount', async () => {
    const { app } = await mountWallet()
    let resolve!: (value: ReturnType<typeof orderResponse>) => void
    walletApiMock.listRechargeOrders.mockReturnValue(new Promise(complete => { resolve = complete }))
    await vi.advanceTimersByTimeAsync(5_000)
    app.unmount()
    mountedApps.splice(0).forEach(({ root }) => root.remove())
    resolve(orderResponse([paymentOrder()]))
    await flushPromises()
    expect(vi.getTimerCount()).toBe(0)
  })
})

describe('WalletCenter recharge recovery', () => {
  beforeEach(() => {
    walletApiMock.listRechargeOrders.mockResolvedValue(orderResponse([paymentOrder('credited')]))
  })

  it('shows separate recharge snapshots without adding debts or exposing internal fields', async () => {
    walletApiMock.listRechargeRecoveries.mockResolvedValue({ items: [
      recovery({ collected_cost_units: 300_000_000, outstanding_cost_units: 400_000_000,
        available_recharge_cost_units: 700_000_000, error_code: 'private-database-error', retry_count: 99 }),
      recovery({ id: 'private-job-two', payment_order_id: 'private-other-order',
        principal_cost_units: 500_000_000, collected_cost_units: 100_000_000,
        outstanding_cost_units: 300_000_000, available_recharge_cost_units: 400_000_000 }),
    ], limit: 50 })
    const { root } = await mountWallet()
    const panel = root.querySelector<HTMLElement>('[data-recharge-recoveries]')!
    const rows = panel.querySelectorAll('[data-recovery-row]')
    expect(rows).toHaveLength(2)
    expect(rows[0].textContent).toContain('RECHARGE-1')
    expect(rows[0].textContent).toContain('$10.00')
    expect(rows[1].textContent).toContain('充值入账')
    expect(panel.textContent).toContain('本次充值对应剩余欠费')
    expect(panel.textContent).toContain('本次追扣后可用本金')
    expect(panel.textContent).toContain('下次处理')
    for (const value of ['private-wallet-id', 'private-job-id', 'private-other-order', 'private-database-error']) {
      expect(root.textContent).not.toContain(value)
    }
    // The snapshot's $7 principal must not replace the real $2 wallet balance.
    expect(root.textContent).toContain('钱包余额: $2.00')
  })

  it.each(['pending', 'retry', 'manual_review', 'source_unavailable'] as const)('marks unverified %s snapshot balances as pending review', async (state) => {
    walletApiMock.listRechargeRecoveries.mockResolvedValue({ items: [recovery({
      state, collected_cost_units: 300_000_000, outstanding_cost_units: 0,
      available_recharge_cost_units: 0, next_attempt_at_unix_secs: null,
    })], limit: 50 })
    const { root } = await mountWallet()
    const row = root.querySelector<HTMLElement>('[data-recovery-row]')!
    expect(row.textContent).toContain('$10.00')
    expect(row.textContent).toContain('$3.00')
    expect(row.textContent?.match(/待核对/g)).toHaveLength(2)
    expect(row.textContent).not.toContain('$0.00')
    expect(root.textContent).toContain('钱包余额: $2.00')
  })

  it('keeps polling after credit and refreshes balance and recovery history after collection', async () => {
    walletApiMock.listRechargeRecoveries.mockResolvedValue({ items: [recovery()], limit: 50 })
    const { root } = await mountWallet()
    const initialFlowCalls = walletApiMock.getFlow.mock.calls.length
    walletApiMock.listRechargeRecoveries.mockResolvedValue({ items: [recovery({
      state: 'completed', collected_cost_units: 700_000_000, outstanding_cost_units: 0,
      available_recharge_cost_units: 300_000_000, next_attempt_at_unix_secs: null,
      updated_at_unix_secs: 1_800_000_001,
    })], limit: 50 })
    walletApiMock.getBalance.mockResolvedValue(walletBalance(5))
    walletApiMock.getFlow.mockResolvedValue({ items: [{ type: 'transaction', data: {
      id: 'recovery-ledger-entry', category: 'adjustment', reason_code: 'historical_debt_recovery',
      amount: -7, balance_before: 12, balance_after: 5,
      recharge_balance_before: 12, recharge_balance_after: 5,
      gift_balance_before: 0, gift_balance_after: 0,
      description: '充值后追扣历史欠费', created_at: '2026-09-17T01:00:00Z',
    } }], total: 1, today_entry: null })
    await vi.advanceTimersByTimeAsync(5_000)
    expect(walletApiMock.getFlow).toHaveBeenCalledTimes(initialFlowCalls + 1)
    expect(root.textContent).toContain('钱包余额: $5.00')
    expect(root.textContent).toContain('充值后追扣历史欠费')
    expect(root.textContent).toContain('历史欠费追扣')
    expect(root.textContent).toContain('-7.0000')
    const calls = walletApiMock.listRechargeRecoveries.mock.calls.length
    await vi.advanceTimersByTimeAsync(15_000)
    expect(walletApiMock.listRechargeRecoveries).toHaveBeenCalledTimes(calls)
    expect(walletApiMock.listRechargeOrders).toHaveBeenCalledTimes(1)
  })

  it('retains the last snapshot through a retry without claiming the debt is cleared', async () => {
    walletApiMock.listRechargeRecoveries.mockResolvedValue({ items: [recovery()], limit: 50 })
    const { root } = await mountWallet()
    walletApiMock.listRechargeRecoveries.mockRejectedValueOnce(new Error('temporary outage'))
    await vi.advanceTimersByTimeAsync(5_000)
    const panel = root.querySelector<HTMLElement>('[data-recharge-recoveries]')!
    expect(panel.textContent).toContain('暂时无法刷新')
    expect(panel.textContent).toContain('RECHARGE-1')
    expect(panel.textContent).toContain('$10.00')
    expect(panel.textContent?.match(/待核对/g)).toHaveLength(2)
    expect(toastMock.error).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(5_000)
    expect(panel.textContent).not.toContain('暂时无法刷新')
    expect(walletApiMock.listRechargeRecoveries).toHaveBeenCalledTimes(3)
  })

  it('does not let an older order response overwrite the balance read after recovery', async () => {
    walletApiMock.listRechargeRecoveries.mockResolvedValue({ items: [recovery()], limit: 50 })
    const { root } = await mountWallet()
    let resolveOrders!: (value: ReturnType<typeof orderResponse>) => void
    walletApiMock.listRechargeOrders.mockReturnValueOnce(new Promise(resolve => { resolveOrders = resolve }))
    root.querySelector<HTMLButtonElement>('[value="orders"] [data-refresh]')!.click()
    await flushPromises()
    walletApiMock.listRechargeRecoveries.mockResolvedValue({ items: [recovery({
      state: 'completed', collected_cost_units: 700_000_000,
      outstanding_cost_units: 0, next_attempt_at_unix_secs: null,
    })], limit: 50 })
    walletApiMock.getBalance.mockResolvedValue(walletBalance(5))
    await vi.advanceTimersByTimeAsync(5_000)
    resolveOrders(orderResponse([paymentOrder('credited')], 12))
    await flushPromises()
    expect(root.textContent).toContain('钱包余额: $5.00')
    expect(root.textContent).not.toContain('钱包余额: $12.00')
  })

  it('retries a failed history refresh even when the recovery job has completed', async () => {
    walletApiMock.listRechargeRecoveries.mockResolvedValue({ items: [recovery()], limit: 50 })
    const { root } = await mountWallet()
    walletApiMock.listRechargeRecoveries.mockResolvedValue({ items: [recovery({
      state: 'completed', collected_cost_units: 700_000_000,
      outstanding_cost_units: 0, next_attempt_at_unix_secs: null,
    })], limit: 50 })
    walletApiMock.getFlow.mockRejectedValueOnce(new Error('history temporarily unavailable'))
    await vi.advanceTimersByTimeAsync(5_000)
    expect(root.querySelector('[data-recharge-recoveries]')!.textContent).toContain('暂时无法刷新')
    expect(toastMock.error).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(5_000)
    expect(walletApiMock.getFlow).toHaveBeenCalledTimes(3)
    expect(root.querySelector('[data-recharge-recoveries]')!.textContent).not.toContain('暂时无法刷新')
  })

  it('shows partially collected debt awaiting the next recharge without continuing an active-job poll', async () => {
    walletApiMock.listRechargeRecoveries.mockResolvedValue({ items: [recovery({
      state: 'waiting_next_recharge', collected_cost_units: 300_000_000,
      outstanding_cost_units: 400_000_000, available_recharge_cost_units: 0,
      next_attempt_at_unix_secs: null,
    })], limit: 50 })
    const { root } = await mountWallet()
    const panel = root.querySelector('[data-recharge-recoveries]')!
    expect(panel.textContent).toContain('等待下次充值')
    expect(panel.textContent).toContain('$4.00')
    expect(panel.textContent).not.toContain('本次处理完成')
    await vi.advanceTimersByTimeAsync(30_000)
    expect(walletApiMock.listRechargeRecoveries).toHaveBeenCalledTimes(1)
  })

  it('throttles long retry waits while keeping a visible status and scheduled processing time', async () => {
    walletApiMock.listRechargeRecoveries.mockResolvedValue({ items: [recovery({
      state: 'retry', next_attempt_at_unix_secs: Math.floor(Date.now() / 1000) + 86_400,
    })], limit: 50 })
    const { root } = await mountWallet()
    expect(root.querySelector('[data-recharge-recoveries]')!.textContent).toContain('等待重试')
    await vi.advanceTimersByTimeAsync(29_000)
    expect(walletApiMock.listRechargeRecoveries).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(1_000)
    expect(walletApiMock.listRechargeRecoveries).toHaveBeenCalledTimes(2)
  })

  it('pauses while hidden and does not revive polling or apply late results after unmount', async () => {
    walletApiMock.listRechargeRecoveries.mockResolvedValue({ items: [recovery()], limit: 50 })
    const { app } = await mountWallet()
    setHidden(true)
    await vi.advanceTimersByTimeAsync(20_000)
    expect(walletApiMock.listRechargeRecoveries).toHaveBeenCalledTimes(1)
    let resolve!: (value: { items: WalletRechargeRecovery[]; limit: number }) => void
    walletApiMock.listRechargeRecoveries.mockReturnValue(new Promise(complete => { resolve = complete }))
    setHidden(false)
    await flushPromises()
    const balanceCalls = walletApiMock.getBalance.mock.calls.length
    app.unmount()
    mountedApps.splice(0).forEach(({ root }) => root.remove())
    resolve({ items: [recovery({ collected_cost_units: 100_000_000 })], limit: 50 })
    await flushPromises()
    expect(walletApiMock.getBalance).toHaveBeenCalledTimes(balanceCalls)
    expect(vi.getTimerCount()).toBe(0)
  })

  it('keeps an unknown state visible without presenting it as completed', async () => {
    walletApiMock.listRechargeRecoveries.mockResolvedValue({ items: [recovery({
      state: 'future_state', next_attempt_at_unix_secs: null,
    })], limit: 50 })
    const { root } = await mountWallet()
    expect(root.querySelector('[data-recharge-recoveries]')!.textContent).toContain('状态待确认')
  })
})
