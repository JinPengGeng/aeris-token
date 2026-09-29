//! 管理端内置导航项可见性配置（issue #573）。
//!
//! 扩展模块系统之外，内置管理菜单项（套餐管理、钱包管理等约 18 个硬编码项）
//! 也支持管理员逐项隐藏。可见性持久化为单条 system_config：
//! `admin.nav.hidden_items`（JSON 字符串数组，元素为导航项 key）。
//! 未写入该配置时隐藏清单为空，所有内置项照常显示（默认行为零改变）。
//!
//! 隐藏仅作用于侧边栏菜单与面包屑展示；路由注册与管理端权限完全不变，
//! 隐藏项仍可通过 URL 深链接直接访问。

/// 隐藏清单的 system_config key（值：`["billingManagement", ...]`）。
pub(crate) const ADMIN_NAVIGATION_HIDDEN_ITEMS_CONFIG_KEY: &str = "admin.nav.hidden_items";

pub(crate) struct AdminNavigationItemDefinition {
    pub(crate) key: &'static str,
    pub(crate) href: &'static str,
    pub(crate) menu_group: &'static str,
    pub(crate) display_name: &'static str,
}

/// 内置管理导航项的可隐藏清单。
///
/// **与前端镜像同步维护**：key 必须与
/// `frontend/src/layouts/main-layout/navigation.ts` 中的
/// `BUILTIN_ADMIN_NAV_ITEM_KEYS`（href -> key）保持一致；修改任一侧时需同步另一侧。
///
/// 刻意不纳入：`dashboard`（登录后落地页）、`moduleManagement`（承载本功能
/// 的管理页入口）、`systemSettings`（系统设置入口）——保留这三个恢复入口，
/// 避免管理员误操作后失去配置界面。
pub(crate) const ADMIN_NAVIGATION_ITEM_DEFINITIONS: &[AdminNavigationItemDefinition] = &[
    AdminNavigationItemDefinition {
        key: "operations",
        href: "/admin/operations",
        menu_group: "overview",
        display_name: "运维总览",
    },
    AdminNavigationItemDefinition {
        key: "healthMonitor",
        href: "/admin/health-monitor",
        menu_group: "overview",
        display_name: "健康监控",
    },
    AdminNavigationItemDefinition {
        key: "userStats",
        href: "/admin/user-stats",
        menu_group: "overview",
        display_name: "用户统计",
    },
    AdminNavigationItemDefinition {
        key: "costAnalysis",
        href: "/admin/cost-analysis",
        menu_group: "overview",
        display_name: "成本分析",
    },
    AdminNavigationItemDefinition {
        key: "marginReport",
        href: "/admin/margin-report",
        menu_group: "overview",
        display_name: "毛利报表",
    },
    AdminNavigationItemDefinition {
        key: "performanceAnalysis",
        href: "/admin/performance-analysis",
        menu_group: "overview",
        display_name: "性能分析",
    },
    AdminNavigationItemDefinition {
        key: "userManagement",
        href: "/admin/users",
        menu_group: "management",
        display_name: "用户管理",
    },
    AdminNavigationItemDefinition {
        key: "providers",
        href: "/admin/providers",
        menu_group: "management",
        display_name: "提供商",
    },
    AdminNavigationItemDefinition {
        key: "modelManagement",
        href: "/admin/models",
        menu_group: "management",
        display_name: "模型管理",
    },
    AdminNavigationItemDefinition {
        key: "routing",
        href: "/admin/routing",
        menu_group: "management",
        display_name: "调度策略",
    },
    AdminNavigationItemDefinition {
        key: "pool",
        href: "/admin/pool",
        menu_group: "management",
        display_name: "号池管理",
    },
    AdminNavigationItemDefinition {
        key: "standaloneKeys",
        href: "/admin/keys",
        menu_group: "management",
        display_name: "独立密钥",
    },
    AdminNavigationItemDefinition {
        key: "walletManagement",
        href: "/admin/wallets",
        menu_group: "management",
        display_name: "钱包管理",
    },
    AdminNavigationItemDefinition {
        key: "billingManagement",
        href: "/admin/billing-plans",
        menu_group: "management",
        display_name: "套餐管理",
    },
    AdminNavigationItemDefinition {
        key: "asyncTasks",
        href: "/admin/async-tasks",
        menu_group: "management",
        display_name: "异步任务",
    },
    AdminNavigationItemDefinition {
        key: "usageRecords",
        href: "/admin/usage",
        menu_group: "management",
        display_name: "使用记录",
    },
    AdminNavigationItemDefinition {
        key: "announcements",
        href: "/admin/announcements",
        menu_group: "system",
        display_name: "公告管理",
    },
    AdminNavigationItemDefinition {
        key: "cacheMonitoring",
        href: "/admin/cache-monitoring",
        menu_group: "system",
        display_name: "缓存监控",
    },
];

pub(crate) fn admin_navigation_item_by_key(
    key: &str,
) -> Option<&'static AdminNavigationItemDefinition> {
    ADMIN_NAVIGATION_ITEM_DEFINITIONS
        .iter()
        .find(|item| item.key == key)
}

/// 将 system_config 中存储的隐藏清单归一化为有效 key 列表。
///
/// 容忍历史脏数据：非数组、非字符串、空白、重复与未知 key（例如某次升级
/// 后被移除的导航项）一律跳过，保持写入顺序。`None`（未配置）返回空清单。
pub(crate) fn stored_admin_navigation_hidden_items(
    stored: Option<&serde_json::Value>,
) -> Vec<String> {
    let Some(value) = stored.and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    let mut items = Vec::new();
    for entry in value {
        let Some(raw) = entry.as_str() else {
            continue;
        };
        let key = raw.trim();
        if key.is_empty() || !seen.insert(key.to_string()) {
            continue;
        }
        if admin_navigation_item_by_key(key).is_some() {
            items.push(key.to_string());
        }
    }
    items
}

/// 校验 PUT 请求体中的隐藏清单，返回归一化后的 key 列表。
///
/// 与存储侧不同，这里对“未知 key”严格报错（防止拼写错误静默丢配置），
/// 对重复 key 宽松去重。
pub(crate) fn parse_admin_navigation_hidden_items(
    value: Option<&serde_json::Value>,
) -> Result<Vec<String>, (axum::http::StatusCode, serde_json::Value)> {
    let Some(value) = value else {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            serde_json::json!({ "detail": "请求体必须包含 hidden_items 字段" }),
        ));
    };
    let Some(entries) = value.as_array() else {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            serde_json::json!({ "detail": "hidden_items 必须为字符串数组" }),
        ));
    };

    let mut seen = std::collections::HashSet::new();
    let mut items = Vec::new();
    for entry in entries {
        let Some(raw) = entry.as_str() else {
            return Err((
                axum::http::StatusCode::BAD_REQUEST,
                serde_json::json!({ "detail": "hidden_items 必须为字符串数组" }),
            ));
        };
        let key = raw.trim();
        if key.is_empty() {
            return Err((
                axum::http::StatusCode::BAD_REQUEST,
                serde_json::json!({ "detail": "hidden_items 不能包含空字符串" }),
            ));
        }
        if admin_navigation_item_by_key(key).is_none() {
            return Err((
                axum::http::StatusCode::BAD_REQUEST,
                serde_json::json!({
                    "detail": format!("未知的导航项 key: {key}")
                }),
            ));
        }
        if seen.insert(key.to_string()) {
            items.push(key.to_string());
        }
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::{
        admin_navigation_item_by_key, parse_admin_navigation_hidden_items,
        stored_admin_navigation_hidden_items, ADMIN_NAVIGATION_HIDDEN_ITEMS_CONFIG_KEY,
        ADMIN_NAVIGATION_ITEM_DEFINITIONS,
    };
    use serde_json::json;

    #[test]
    fn hidden_items_config_key_is_stable() {
        assert_eq!(
            ADMIN_NAVIGATION_HIDDEN_ITEMS_CONFIG_KEY,
            "admin.nav.hidden_items"
        );
    }

    #[test]
    fn item_catalog_covers_the_hardcoded_builtin_nav_items() {
        let keys: Vec<_> = ADMIN_NAVIGATION_ITEM_DEFINITIONS
            .iter()
            .map(|i| i.key)
            .collect();
        for key in [
            "billingManagement",
            "walletManagement",
            "announcements",
            "cacheMonitoring",
            "marginReport",
            "performanceAnalysis",
            "operations",
            "pool",
            "standaloneKeys",
            "asyncTasks",
            "usageRecords",
        ] {
            assert!(keys.contains(&key), "item catalog should contain {key}");
        }
        // 恢复入口不纳入可隐藏清单。
        for protected in ["dashboard", "moduleManagement", "systemSettings"] {
            assert!(
                admin_navigation_item_by_key(protected).is_none(),
                "{protected} must stay reachable and not be hideable"
            );
        }
        assert_eq!(ADMIN_NAVIGATION_ITEM_DEFINITIONS.len(), 18);
    }

    #[test]
    fn item_hrefs_are_unique_and_admin_scoped() {
        let mut hrefs: Vec<_> = ADMIN_NAVIGATION_ITEM_DEFINITIONS
            .iter()
            .map(|i| i.href)
            .collect();
        hrefs.sort_unstable();
        let total = hrefs.len();
        hrefs.dedup();
        assert_eq!(hrefs.len(), total, "hrefs must be unique");
        assert!(hrefs.iter().all(|href| href.starts_with("/admin/")));
    }

    #[test]
    fn stored_hidden_items_default_to_empty_without_config() {
        assert!(stored_admin_navigation_hidden_items(None).is_empty());
        assert!(stored_admin_navigation_hidden_items(Some(&serde_json::Value::Null)).is_empty());
    }

    #[test]
    fn stored_hidden_items_tolerate_malformed_values() {
        assert!(stored_admin_navigation_hidden_items(Some(&json!("billingManagement"))).is_empty());
        assert!(stored_admin_navigation_hidden_items(Some(&json!([1, true, null]))).is_empty());
    }

    #[test]
    fn stored_hidden_items_filter_unknown_keys_and_deduplicate() {
        let stored = json!([
            "billingManagement",
            "unknown-item",
            "billingManagement",
            "  pool  ",
            "",
        ]);
        assert_eq!(
            stored_admin_navigation_hidden_items(Some(&stored)),
            vec!["billingManagement".to_string(), "pool".to_string()]
        );
    }

    #[test]
    fn parse_hidden_items_accepts_empty_list_as_show_all() {
        let parsed = parse_admin_navigation_hidden_items(Some(&json!([])))
            .expect("empty list is a valid show-all request");
        assert!(parsed.is_empty());
    }

    #[test]
    fn parse_hidden_items_normalizes_and_deduplicates() {
        let parsed = parse_admin_navigation_hidden_items(Some(&json!([
            "billingManagement",
            " pool ",
            "billingManagement"
        ])))
        .expect("valid request should parse");
        assert_eq!(
            parsed,
            vec!["billingManagement".to_string(), "pool".to_string()]
        );
    }

    #[test]
    fn parse_hidden_items_rejects_unknown_keys() {
        let error = parse_admin_navigation_hidden_items(Some(&json!(["not-a-nav-item"])))
            .expect_err("unknown key must be rejected");
        assert_eq!(error.0, axum::http::StatusCode::BAD_REQUEST);
        assert!(error.1["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("not-a-nav-item"));
    }

    #[test]
    fn parse_hidden_items_rejects_malformed_payloads() {
        assert!(parse_admin_navigation_hidden_items(None).is_err());
        assert!(parse_admin_navigation_hidden_items(Some(&json!("pool"))).is_err());
        assert!(parse_admin_navigation_hidden_items(Some(&json!(["pool", 1]))).is_err());
        assert!(parse_admin_navigation_hidden_items(Some(&json!([" "]))).is_err());
    }
}
