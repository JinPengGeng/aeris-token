import { beforeEach, describe, expect, it, vi } from 'vitest'

const { postMock } = vi.hoisted(() => ({
  postMock: vi.fn(),
}))

vi.mock('@/api/client', () => ({
  default: {
    post: postMock,
    get: vi.fn(),
    put: vi.fn(),
    delete: vi.fn(),
  },
}))

import { syncRemoteQuota } from '@/api/providerOps'
import type { ActionResultResponse } from '@/api/providerOps'

function actionPayload(status: string, message: string | null = null): ActionResultResponse {
  return {
    status: status as ActionResultResponse['status'],
    action_type: 'sync_remote_quota',
    data: status === 'success'
      ? { attempted: 1, applied: 1, blocked: 0, recovered: 0, skipped: 0, failed: 0, last_error: null }
      : null,
    message,
    executed_at: '2026-09-07T00:00:00Z',
    response_time_ms: 12,
    cache_ttl_seconds: 0,
  }
}

describe('syncRemoteQuota', () => {
  beforeEach(() => {
    postMock.mockReset()
  })

  it('returns the action payload on HTTP 200', async () => {
    postMock.mockResolvedValueOnce({ data: actionPayload('success') })

    const result = await syncRemoteQuota('provider-1')

    expect(postMock).toHaveBeenCalledWith(
      '/api/admin/provider-ops/providers/provider-1/actions/sync_remote_quota',
      {},
    )
    expect(result.status).toBe('success')
    expect(result.action_type).toBe('sync_remote_quota')
  })

  it('normalizes a 400 not_configured rejection into the action payload', async () => {
    postMock.mockRejectedValueOnce({
      isAxiosError: true,
      response: {
        status: 400,
        data: actionPayload('not_configured', '该 Provider 未启用远程配额同步（provider_ops.remote_quota.enabled）'),
      },
    })

    const result = await syncRemoteQuota('provider-disabled')

    expect(result.status).toBe('not_configured')
    expect(result.message).toContain('未启用远程配额同步')
  })

  it('rethrows non-action HTTP errors', async () => {
    const serverError = {
      isAxiosError: true,
      response: { status: 500, data: { detail: 'internal' } },
    }
    postMock.mockRejectedValueOnce(serverError)

    await expect(syncRemoteQuota('provider-1')).rejects.toBe(serverError)
  })

  it('rethrows transport errors without a response', async () => {
    const networkError = { isAxiosError: true, message: 'Network Error' }
    postMock.mockRejectedValueOnce(networkError)

    await expect(syncRemoteQuota('provider-1')).rejects.toBe(networkError)
  })
})
