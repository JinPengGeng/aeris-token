import { describe, expect, it } from 'vitest'

import { dashboardRoutes } from '../dashboard'

describe('dashboardRoutes module meta', () => {
  it('为 /dashboard/referral 标注 referral 模块，直达刷新时可触发模块状态按需加载', () => {
    const children = dashboardRoutes[0]?.children ?? []
    const referral = children.find(child => child.path === 'referral')

    expect(referral?.meta).toMatchObject({ module: 'referral' })
  })

  it('用户侧带模块要求的路由与入口条件使用同一 module 标识', () => {
    const children = dashboardRoutes[0]?.children ?? []
    const managementTokens = children.find(child => child.path === 'management-tokens')

    expect(managementTokens?.meta).toMatchObject({ module: 'management_tokens' })
  })
})
