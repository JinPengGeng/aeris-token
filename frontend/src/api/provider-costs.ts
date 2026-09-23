import apiClient from './client'

export type ProviderCostTaskType = 'text' | 'image'

/** 成本价目 catalog 与售价 catalog(BillingModelPricingSnapshot)JSON 形状同构。 */
export interface ProviderCostCatalogRecord {
  cost_id: string
  provider_id: string
  model: string
  task_type: ProviderCostTaskType
  currency: string
  price_per_request: number | null
  tiered_pricing: Record<string, unknown> | null
  effective_from_unix_secs: number
  effective_to_unix_secs: number | null
  created_by: string
  created_at_unix_secs: number
  updated_at_unix_secs: number
}

export interface ProviderCostCatalogWriteRequest {
  provider_id: string
  model: string
  task_type: ProviderCostTaskType
  currency?: string
  price_per_request?: number | null
  tiered_pricing?: Record<string, unknown> | null
  effective_from_unix_secs: number
  effective_to_unix_secs?: number | null
}

export interface ProviderCostCatalogListParams {
  provider_id?: string
  model?: string
  task_type?: ProviderCostTaskType
  effective_at?: number
  page?: number
  page_size?: number
}

const BASE_URL = '/api/admin/billing/provider-cost-catalogs'

export async function listProviderCostCatalogs(
  params: ProviderCostCatalogListParams = {}
): Promise<ProviderCostCatalogRecord[]> {
  const response = await apiClient.get<{ items: ProviderCostCatalogRecord[] }>(BASE_URL, { params })
  return response.data.items
}

export async function findEffectiveProviderCostCatalog(
  providerId: string,
  model: string,
  taskType: ProviderCostTaskType,
  at: number
): Promise<ProviderCostCatalogRecord | null> {
  const response = await apiClient.get<{ item: ProviderCostCatalogRecord }>(`${BASE_URL}/effective`, {
    params: {
      provider_id: providerId,
      model,
      task_type: taskType,
      at,
    },
  })
  return response.data.item ?? null
}

export async function createProviderCostCatalog(
  payload: ProviderCostCatalogWriteRequest
): Promise<void> {
  await apiClient.post(BASE_URL, payload)
}

export async function updateProviderCostCatalog(
  costId: string,
  payload: ProviderCostCatalogWriteRequest
): Promise<void> {
  await apiClient.put(`${BASE_URL}/${costId}`, payload)
}

export async function deleteProviderCostCatalog(costId: string): Promise<void> {
  await apiClient.delete(`${BASE_URL}/${costId}`)
}
