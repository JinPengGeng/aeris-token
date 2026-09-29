import { describe, expect, it } from 'vitest'
import type { RouteLocationNormalizedLoaded } from 'vue-router'

import { BUILTIN_ADMIN_NAV_ITEM_KEYS, buildBreadcrumbs, buildNavigation, isItemHidden } from '@/layouts/main-layout/navigation'
import type { MessageKey } from '@/i18n'

const translate = (key: MessageKey) => `tx:${key}`

function route(path: string, name?: string, meta: Record<string, unknown> = {}): RouteLocationNormalizedLoaded {
  return {
    path,
    fullPath: path,
    query: {},
    hash: '',
    name,
    params: {},
    matched: [],
    meta,
    redirectedFrom: undefined,
  } as RouteLocationNormalizedLoaded
}

describe('main layout navigation builder', () => {
  it('builds user navigation from translation keys and active modules', () => {
    const navigation = buildNavigation({
      canAccessAdmin: false,
      modules: {},
      isModuleActive: (name) => name === 'referral',
      t: translate,
    })

    expect(navigation.map(group => group.title)).toEqual([
      'tx:nav.group.overview',
      'tx:nav.group.resources',
      'tx:nav.group.account',
    ])
    expect(navigation.flatMap(group => group.items.map(item => item.name))).toContain('tx:nav.myReferral')
  })

  it('exposes remote control to users and active administrators', () => {
    const userNavigation = buildNavigation({
      canAccessAdmin: false,
      modules: {},
      isModuleActive: () => false,
      t: translate,
    })
    const adminNavigation = buildNavigation({
      canAccessAdmin: true,
      modules: {
        vscodex: {
          active: true,
          name: 'test-module',
          available: true,
          enabled: true,
          config_validated: true,
          config_error: null,
          description: '',
          category: 'integration',
          health: 'healthy',
          admin_route: '/dashboard/vscodex',
          admin_menu_group: 'overview',
          admin_menu_order: 80,
          admin_menu_icon: 'SquareTerminal',
          display_name: '远程控制',
        },
      },
      isModuleActive: () => false,
      t: translate,
    })

    const findVscodeControl = (navigation: ReturnType<typeof buildNavigation>) => (
      navigation
        .flatMap(group => group.items)
        .find(item => item.href === '/dashboard/vscodex')
    )

    expect(findVscodeControl(userNavigation)).toMatchObject({
      name: 'tx:nav.vscodex',
      href: '/dashboard/vscodex',
    })
    expect(findVscodeControl(adminNavigation)).toMatchObject({
      name: '远程控制',
      href: '/dashboard/vscodex',
    })

    const overviewItems = adminNavigation.find(group => group.title === 'tx:nav.group.overview')?.items ?? []
    expect(overviewItems.findIndex(item => item.name === '远程控制')).toBe(
      overviewItems.findIndex(item => item.name === 'tx:nav.performanceAnalysis') + 1,
    )
  })

  it('builds admin navigation with dynamic module menu items sorted by menu order', () => {
    const navigation = buildNavigation({
      canAccessAdmin: true,
      modules: {
        first: {
          active: true,
          name: 'test-module',
          available: true,
          enabled: true,
          config_validated: true,
          config_error: null,
          description: '',
          category: 'integration',
          health: 'healthy',
          admin_route: '/admin/first',
          admin_menu_group: 'management',
          admin_menu_order: 2,
          admin_menu_icon: 'Gift',
          display_name: 'First module',
        },
        second: {
          active: true,
          name: 'test-module',
          available: true,
          enabled: true,
          config_validated: true,
          config_error: null,
          description: '',
          category: 'integration',
          health: 'healthy',
          admin_route: '/admin/second',
          admin_menu_group: 'management',
          admin_menu_order: 1,
          admin_menu_icon: 'Key',
          display_name: 'Second module',
        },
      },
      isModuleActive: () => false,
      t: translate,
    })

    const managementItems = navigation.find(group => group.title === 'tx:nav.group.management')?.items ?? []
    expect(managementItems.map(item => item.name)).toEqual(expect.arrayContaining(['Second module', 'First module']))
    expect(managementItems.findIndex(item => item.name === 'Second module')).toBeLessThan(
      managementItems.findIndex(item => item.name === 'First module')
    )
  })

  it('shows all builtin items by default (no isItemHidden predicate)', () => {
    const navigation = buildNavigation({
      canAccessAdmin: true,
      modules: {},
      isModuleActive: () => false,
      t: translate,
    })

    const hrefs = navigation.flatMap(group => group.items.map(item => item.href))
    for (const href of Object.keys(BUILTIN_ADMIN_NAV_ITEM_KEYS)) {
      expect(hrefs).toContain(href)
    }
  })

  it('filters hidden builtin nav items from admin menu while keeping protected entries', () => {
    const navigation = buildNavigation({
      canAccessAdmin: true,
      modules: {},
      isModuleActive: () => false,
      isItemHidden: key => key === 'billingManagement' || key === 'marginReport' || key === 'announcements',
      t: translate,
    })

    const hrefs = navigation.flatMap(group => group.items.map(item => item.href))
    expect(hrefs).not.toContain('/admin/billing-plans')
    expect(hrefs).not.toContain('/admin/margin-report')
    expect(hrefs).not.toContain('/admin/announcements')
    // 隐藏仅作用于菜单展示：受保护入口与扩展模块项不受影响。
    expect(hrefs).toContain('/admin/dashboard')
    expect(hrefs).toContain('/admin/modules')
    expect(hrefs).toContain('/admin/system')
  })

  it('does not hide module-provided menu entries even when hrefs overlap keys', () => {
    const navigation = buildNavigation({
      canAccessAdmin: true,
      modules: {
        referral: {
          active: true,
          name: 'referral',
          available: true,
          enabled: true,
          config_validated: true,
          config_error: null,
          description: '',
          category: 'integration',
          health: 'healthy',
          admin_route: '/admin/referrals',
          admin_menu_group: 'management',
          admin_menu_order: 75,
          admin_menu_icon: 'Gift',
          display_name: '邀请返利',
        },
      },
      isModuleActive: () => true,
      isItemHidden: () => true,
      t: translate,
    })

    const hrefs = navigation.flatMap(group => group.items.map(item => item.href))
    expect(hrefs).toContain('/admin/referrals')
    // 内置项全部隐藏，仅剩扩展模块项与未列入镜像表的条目。
    const builtinHrefs = Object.keys(BUILTIN_ADMIN_NAV_ITEM_KEYS)
    for (const href of builtinHrefs) {
      expect(hrefs).not.toContain(href)
    }
  })

  it('mirrors the backend hideable item catalog (18 items, protected anchors excluded)', () => {
    const keys = Object.values(BUILTIN_ADMIN_NAV_ITEM_KEYS)
    expect(keys).toHaveLength(18)
    expect(new Set(keys).size).toBe(18)
    for (const protectedKey of ['dashboard', 'moduleManagement', 'systemSettings']) {
      expect(keys).not.toContain(protectedKey)
    }
  })

  it('isItemHidden matches keys contained in the hidden list only', () => {
    expect(isItemHidden(['billingManagement'], 'billingManagement')).toBe(true)
    expect(isItemHidden(['billingManagement'], 'pool')).toBe(false)
    expect(isItemHidden([], 'billingManagement')).toBe(false)
  })

  it('builds translated breadcrumbs for settings and routing detail pages', () => {
    const navigation = buildNavigation({
      canAccessAdmin: true,
      modules: {},
      isModuleActive: () => false,
      t: translate,
    })

    expect(buildBreadcrumbs({
      route: route('/dashboard/settings'),
      navigation,
      modules: {},
      isNavActive: () => false,
      t: translate,
    })).toEqual([
      { label: 'tx:nav.group.account' },
      { label: 'tx:breadcrumb.personalSettings' },
    ])

    expect(buildBreadcrumbs({
      route: route('/admin/routing/new', 'RoutingProfileCreate'),
      navigation,
      modules: {},
      isNavActive: href => href === '/admin/routing',
      t: translate,
    })).toEqual([
      { label: 'tx:nav.group.management' },
      { label: 'tx:nav.routing', href: '/admin/routing' },
      { label: 'tx:breadcrumb.routingCreate' },
    ])

    // 隐藏项不再出现在导航里，对应路由的面包屑回退到默认兜底。
    expect(buildBreadcrumbs({
      route: route('/admin/billing-plans'),
      navigation: buildNavigation({
        canAccessAdmin: true,
        modules: {},
        isModuleActive: () => false,
        isItemHidden: key => key === 'billingManagement',
        t: translate,
      }),
      modules: {},
      isNavActive: href => href === '/admin/billing-plans',
      t: translate,
    })).toEqual([{ label: 'tx:nav.dashboard' }])

    expect(buildBreadcrumbs({
      route: route('/dashboard/vscodex'),
      navigation: buildNavigation({
        canAccessAdmin: true,
        modules: {
          vscodex: {
            active: true,
          name: 'test-module',
          available: true,
          enabled: true,
          config_validated: true,
          config_error: null,
          description: '',
          category: 'integration',
          health: 'healthy',
            admin_route: '/dashboard/vscodex',
            admin_menu_group: 'overview',
            admin_menu_order: 80,
            admin_menu_icon: 'SquareTerminal',
            display_name: '远程控制',
          },
        },
        isModuleActive: () => false,
        t: translate,
      }),
      modules: {
        vscodex: {
          active: true,
          name: 'test-module',
          available: true,
          enabled: true,
          config_validated: true,
          config_error: null,
          description: '',
          category: 'integration',
          health: 'healthy',
          admin_route: '/dashboard/vscodex',
          admin_menu_group: 'overview',
          admin_menu_order: 80,
          admin_menu_icon: 'SquareTerminal',
          display_name: '远程控制',
        },
      },
      isNavActive: href => href === '/dashboard/vscodex',
      t: translate,
    })).toEqual([
      expect.objectContaining({ label: expect.any(String) }),
      { label: '远程控制' },
    ])
  })
})
