import apiClient from './client'

/** Selected fields from current authorization policy, not historical request permissions. */
export interface ResolvedAuthApiKeySnapshot {
  user_id?: string
  user_is_active?: boolean
  user_is_deleted?: boolean
  user_rate_limit?: number | null
  user_daily_usage_limit_usd?: number | null
  user_allowed_providers?: string[] | null
  user_allowed_api_formats?: string[] | null
  user_allowed_models?: string[] | null
  api_key_id?: string
  api_key_is_active?: boolean
  api_key_is_locked?: boolean
  api_key_expires_at_unix_secs?: number | null
  api_key_rate_limit?: number | null
  api_key_concurrent_limit?: number | null
  api_key_allowed_providers?: string[] | null
  api_key_allowed_api_formats?: string[] | null
  api_key_allowed_models?: string[] | null
  api_key_daily_usage_limit_usd?: number | null
  currently_usable?: boolean
}
export const requestAuthApi = {
  async getCurrentAuthorization(
    userId: string,
    apiKeyId: string,
    signal?: AbortSignal,
  ): Promise<ResolvedAuthApiKeySnapshot> {
    const response = await apiClient.get<ResolvedAuthApiKeySnapshot>(
      `/_gateway/audit/auth/users/${encodeURIComponent(userId)}/api-keys/${encodeURIComponent(apiKeyId)}`,
      { signal },
    )
    return response.data
  },
}
