import apiClient from './client'

export interface WalletSummary {
  id: string
  // balance = 钱包可用余额（充值余额 + 赠款余额），不包含套餐每日额度
  balance: string
  recharge_balance: string
  gift_balance: string
  refundable_balance: string
  currency: string
  status: string
  limit_mode?: 'finite' | 'unlimited'
  unlimited?: boolean
  total_recharged: string
  total_consumed: string
  total_refunded: string
  total_adjusted: string
  updated_at: string
}

export interface WalletDailyQuotaSummary {
  has_active: boolean
  total_usd: string
  used_usd: string
  remaining_usd: string
  allow_wallet_overage: boolean
}

export interface WalletBalanceResponse {
  wallet: WalletSummary | null
  unlimited: boolean
  limit_mode: 'finite' | 'unlimited'
  // balance = 钱包可用余额（充值余额 + 赠款余额），不包含套餐每日额度
  balance: string | null
  recharge_balance?: string | null
  gift_balance?: string | null
  refundable_balance?: string | null
  wallet_balance?: string | null
  package_balance?: string | null
  total_available_balance?: string | null
  daily_quota?: WalletDailyQuotaSummary | null
  deduction_order?: string[]
  currency: string
  pending_refund_count?: number
}

export interface WalletTransaction {
  id: string
  category: string
  reason_code: string
  amount: string
  // 总可用余额（充值+赠款）快照
  balance_before: string
  balance_after: string
  // 分账户快照
  recharge_balance_before: string
  recharge_balance_after: string
  gift_balance_before: string
  gift_balance_after: string
  link_type?: string | null
  link_id?: string | null
  operator_id?: string | null
  operator_name?: string | null
  operator_email?: string | null
  description?: string | null
  created_at: string
}

export interface WalletTransactionsResponse extends WalletBalanceResponse {
  items: WalletTransaction[]
  total: number
  limit: number
  offset: number
}

export interface DailyUsageRecord {
  id?: string | null
  date: string | null
  timezone?: string | null
  total_cost: string
  total_requests: number
  input_tokens: number
  output_tokens: number
  cache_creation_tokens: number
  cache_read_tokens: number
  first_finalized_at?: string | null
  last_finalized_at?: string | null
  aggregated_at?: string | null
  is_today: boolean
}

export type FlowItem =
  | { type: 'transaction'; data: WalletTransaction }
  | { type: 'daily_usage'; data: DailyUsageRecord }

export interface WalletFlowResponse extends WalletBalanceResponse {
  today_entry: DailyUsageRecord | null
  items: FlowItem[]
  total: number
  limit: number
  offset: number
}

export type TodayCostResponse = DailyUsageRecord

export interface PaymentOrder {
  id: string
  order_no: string
  wallet_id: string
  user_id: string | null
  amount_usd: string
  pay_amount: string | null
  pay_currency: string | null
  exchange_rate: number | null
  refunded_amount_usd: string
  refundable_amount_usd: string
  payment_method: string
  payment_provider?: string | null
  payment_channel?: string | null
  order_kind?: 'wallet_recharge' | 'plan_purchase' | string
  product_id?: string | null
  product_snapshot?: Record<string, unknown> | null
  fulfillment_status?: string | null
  fulfillment_error?: string | null
  gateway_order_id: string | null
  gateway_response: Record<string, unknown> | null
  has_gateway_response?: boolean
  status: string
  created_at: string
  paid_at: string | null
  credited_at: string | null
  expires_at: string | null
}

export interface WalletRechargeOrdersResponse extends WalletBalanceResponse {
  items: PaymentOrder[]
  total: number
  limit: number
  offset: number
}

export interface WalletRechargeRecovery {
  id: string
  payment_order_id: string
  wallet_id: string
  state: string
  principal_cost_units: number
  collected_cost_units: number
  outstanding_cost_units: number
  available_recharge_cost_units: number
  retry_count: number
  next_attempt_at_unix_secs: number | null
  error_code: string | null
  created_at_unix_secs: number
  updated_at_unix_secs: number
}

export interface WalletRechargeRecoveriesResponse {
  items: WalletRechargeRecovery[]
  limit: number
}

export interface RefundRequest {
  id: string
  refund_no: string
  payment_order_id: string | null
  source_type: string
  source_id: string | null
  refund_mode: string
  amount_usd: string
  status: string
  reason: string | null
  failure_reason: string | null
  gateway_refund_id: string | null
  payout_method: string | null
  payout_reference: string | null
  payout_proof?: Record<string, unknown> | null
  created_at: string
  updated_at: string
  processed_at: string | null
  completed_at: string | null
}

export interface WalletRechargeCreateRequest {
  amount_usd: string | number
  payment_method: string
  payment_provider?: string
  payment_channel?: string
  pay_amount?: string | number
  pay_currency?: string
  exchange_rate?: number
  idempotency_key?: string
}

export interface WalletRechargeOption {
  payment_method: string
  display_name: string
  provider?: string
  payment_provider?: string
  payment_channel?: string
  pay_currency?: string
  usd_exchange_rate?: number
  min_recharge_usd?: number
  fee_rate?: number
}

export interface WalletRefundCreateRequest {
  amount_usd: string | number
  payment_order_id?: string
  reason?: string
  idempotency_key?: string
}

export interface WalletRefundEligibilityResponse {
  payment_methods: string[]
}

export interface WalletRedeemRequest {
  code: string
}

export interface WalletRedeemResponse {
  order: PaymentOrder
  wallet: WalletSummary
  amount_usd: string
  batch_name: string
}

export const walletApi = {
  async getBalance(): Promise<WalletBalanceResponse> {
    const response = await apiClient.get<WalletBalanceResponse>('/api/wallet/balance')
    return response.data
  },

  async getTransactions(params?: { limit?: number; offset?: number }): Promise<WalletTransactionsResponse> {
    const response = await apiClient.get<WalletTransactionsResponse>('/api/wallet/transactions', { params })
    return response.data
  },

  async getFlow(params?: { limit?: number; offset?: number }): Promise<WalletFlowResponse> {
    const response = await apiClient.get<WalletFlowResponse>('/api/wallet/flow', { params })
    return response.data
  },

  async getTodayCost(): Promise<TodayCostResponse> {
    const response = await apiClient.get<TodayCostResponse>('/api/wallet/today-cost')
    return response.data
  },

  async createRechargeOrder(payload: WalletRechargeCreateRequest): Promise<{
    order: PaymentOrder
    payment_instructions: Record<string, unknown>
  }> {
    const response = await apiClient.post<{
    order: PaymentOrder
    payment_instructions: Record<string, unknown>
  }>('/api/wallet/recharge', payload)
    return response.data
  },

  async listRechargeOptions(): Promise<{ items: WalletRechargeOption[] }> {
    const response = await apiClient.get<{ items: WalletRechargeOption[] }>('/api/wallet/recharge/options')
    return response.data
  },

  async listRechargeOrders(params?: { limit?: number; offset?: number }): Promise<WalletRechargeOrdersResponse> {
    const response = await apiClient.get<WalletRechargeOrdersResponse>('/api/wallet/recharge', { params })
    return response.data
  },

  async getRechargeOrder(orderId: string): Promise<{ order: PaymentOrder }> {
    const response = await apiClient.get<{ order: PaymentOrder }>(`/api/wallet/recharge/${orderId}`)
    return response.data
  },

  async listRechargeRecoveries(): Promise<WalletRechargeRecoveriesResponse> {
    const response = await apiClient.get<WalletRechargeRecoveriesResponse>(
      '/api/wallet/recharge-recoveries', { params: { limit: 50 } },
    )
    return response.data
  },

  async listRefunds(params?: { limit?: number; offset?: number }): Promise<{
    items: RefundRequest[]
    total: number
    limit: number
    offset: number
  }> {
    const response = await apiClient.get<{
    items: RefundRequest[]
    total: number
    limit: number
    offset: number
  }>('/api/wallet/refunds', { params })
    return response.data
  },

  async getRefund(refundId: string): Promise<RefundRequest> {
    const response = await apiClient.get<RefundRequest>(`/api/wallet/refunds/${refundId}`)
    return response.data
  },

  async listRefundEligibleProviders(): Promise<WalletRefundEligibilityResponse> {
    const response = await apiClient.get<WalletRefundEligibilityResponse>('/api/wallet/refunds/eligible-providers')
    return response.data
  },

  async createRefund(payload: WalletRefundCreateRequest): Promise<RefundRequest> {
    const response = await apiClient.post<RefundRequest>('/api/wallet/refunds', payload)
    return response.data
  },

  async redeemCode(payload: WalletRedeemRequest): Promise<WalletRedeemResponse> {
    const response = await apiClient.post<WalletRedeemResponse>('/api/wallet/redeem', payload)
    return response.data
  },
}
