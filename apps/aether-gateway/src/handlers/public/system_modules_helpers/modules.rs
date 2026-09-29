use crate::admin_api::AdminAppState;
use crate::handlers::admin::build_admin_modules_status_payload;
use crate::handlers::shared::{module_available_from_env, system_config_bool};
use crate::{AppState, GatewayError};
use serde_json::json;

#[derive(Clone, Copy)]
struct PublicAuthModuleDefinition {
    name: &'static str,
    display_name: &'static str,
    env_key: &'static str,
    default_available: bool,
}

const PUBLIC_AUTH_MODULE_DEFINITIONS: &[PublicAuthModuleDefinition] = &[
    PublicAuthModuleDefinition {
        name: "oauth",
        display_name: "OAuth 登录",
        env_key: "OAUTH_AVAILABLE",
        default_available: true,
    },
    PublicAuthModuleDefinition {
        name: "ldap",
        display_name: "LDAP 认证",
        env_key: "LDAP_AVAILABLE",
        default_available: true,
    },
];

pub(crate) fn oauth_module_config_is_valid(
    providers: &[aether_data::repository::auth_modules::StoredOAuthProviderModuleConfig],
) -> bool {
    !providers.is_empty()
        && providers.iter().all(|provider| {
            !provider.client_id.trim().is_empty()
                && provider
                    .client_secret_encrypted
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .is_some()
                && !provider.redirect_uri.trim().is_empty()
        })
}

pub(crate) fn ldap_module_config_is_valid(
    config: Option<&aether_data::repository::auth_modules::StoredLdapModuleConfig>,
) -> bool {
    crate::handlers::shared::ldap_module_config_is_valid(config)
}

pub(crate) async fn build_public_auth_modules_status_payload(
    state: &AppState,
) -> Result<serde_json::Value, GatewayError> {
    let oauth_providers = state.list_enabled_oauth_module_providers().await?;
    let ldap_config = state.get_ldap_module_config().await?;
    let oauth_active = oauth_module_config_is_valid(&oauth_providers);
    let ldap_active = ldap_module_config_is_valid(ldap_config.as_ref());

    let mut items = Vec::new();
    for module in PUBLIC_AUTH_MODULE_DEFINITIONS {
        if !module_available_from_env(module.env_key, module.default_available) {
            continue;
        }
        let enabled = state
            .read_system_config_json_value(&format!("module.{}.enabled", module.name))
            .await
            .ok()
            .flatten();
        let enabled = system_config_bool(enabled.as_ref(), false);
        let active = match module.name {
            "oauth" => enabled && oauth_active,
            "ldap" => enabled && ldap_active,
            _ => false,
        };
        items.push(json!({
            "name": module.name,
            "display_name": module.display_name,
            "active": active,
        }));
    }

    Ok(serde_json::Value::Array(items))
}

/// 从管理端模块状态 payload 投影出用户侧只读视图。
///
/// 仅保留 `name` 与 `active`（`active = available && enabled && config_validated`
/// 的最终结果），不泄露 `enabled` / `config_validated` / `config_error` 等内部
/// 配置细节，也不包含管理端菜单路由信息。
pub(crate) fn build_user_modules_status_payload_from_admin(
    admin_payload: serde_json::Value,
) -> serde_json::Value {
    let Some(modules) = admin_payload.as_object() else {
        return serde_json::Value::Object(serde_json::Map::new());
    };
    let mut payload = serde_json::Map::new();
    for (name, status) in modules {
        payload.insert(
            name.clone(),
            json!({
                "name": name,
                "active": status
                    .get("active")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            }),
        );
    }
    serde_json::Value::Object(payload)
}

/// 用户侧只读模块状态：登录用户可访问，复用管理端 `active` 计算逻辑。
pub(crate) async fn build_user_modules_status_payload(
    state: &AppState,
) -> Result<serde_json::Value, GatewayError> {
    let admin_payload = build_admin_modules_status_payload(&AdminAppState::new(state)).await?;
    Ok(build_user_modules_status_payload_from_admin(admin_payload))
}

#[cfg(test)]
mod tests {
    use super::build_user_modules_status_payload_from_admin;
    use serde_json::json;

    #[test]
    fn user_payload_keeps_only_name_and_active() {
        let admin_payload = json!({
            "referral": {
                "name": "referral",
                "available": true,
                "enabled": true,
                "active": true,
                "config_validated": true,
                "config_error": null,
                "display_name": "邀请返利",
                "description": "管理用户邀请关系与返利记录",
                "category": "integration",
                "admin_route": "/admin/referrals",
                "admin_menu_icon": "Gift",
                "admin_menu_group": "management",
                "admin_menu_order": 75,
                "health": "unknown"
            },
            "oauth": {
                "name": "oauth",
                "available": true,
                "enabled": false,
                "active": false,
                "config_validated": true,
                "config_error": null,
                "display_name": "OAuth 登录",
                "description": "支持通过第三方 OAuth Provider 登录/绑定账号",
                "category": "auth",
                "admin_route": "/admin/oauth",
                "admin_menu_icon": "Key",
                "admin_menu_group": null,
                "admin_menu_order": 55,
                "health": "unknown"
            }
        });

        let payload = build_user_modules_status_payload_from_admin(admin_payload);

        assert_eq!(
            payload,
            json!({
                "referral": { "name": "referral", "active": true },
                "oauth": { "name": "oauth", "active": false }
            })
        );
    }

    #[test]
    fn user_payload_defaults_missing_active_to_false() {
        let payload = build_user_modules_status_payload_from_admin(json!({
            "referral": { "name": "referral", "enabled": true }
        }));
        assert_eq!(
            payload,
            json!({ "referral": { "name": "referral", "active": false } })
        );
    }

    #[test]
    fn user_payload_of_non_object_admin_payload_is_empty_object() {
        let payload = build_user_modules_status_payload_from_admin(json!([]));
        assert_eq!(payload, json!({}));
    }

    #[test]
    fn user_payload_of_empty_admin_payload_is_empty_object() {
        let payload = build_user_modules_status_payload_from_admin(json!({}));
        assert_eq!(payload, json!({}));
    }
}
