import { afterEach, describe, expect, it, vi } from 'vitest'
const { get } = vi.hoisted(() => ({ get: vi.fn() }))
vi.mock('@/api/client', () => ({ default: { get } }))
import { requestAuthApi } from '@/api/requestAuth'

afterEach(() => get.mockReset())
describe('current authorization route', () => {
  it('encodes each identity segment and propagates cancellation through the shared authenticated client', async () => {
    const controller = new AbortController()
    const data = { user_id: 'user/1', api_key_id: 'key?#2' }
    get.mockResolvedValue({ data })
    await expect(requestAuthApi.getCurrentAuthorization('user/1', 'key?#2', controller.signal)).resolves.toEqual(data)
    expect(get).toHaveBeenCalledWith('/_gateway/audit/auth/users/user%2F1/api-keys/key%3F%232', { signal: controller.signal })
  })
})
