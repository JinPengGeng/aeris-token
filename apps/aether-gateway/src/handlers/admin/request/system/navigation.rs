use crate::handlers::admin::request::AdminAppState;
use crate::handlers::admin::system::shared::navigation::{
    parse_admin_navigation_hidden_items, stored_admin_navigation_hidden_items,
    ADMIN_NAVIGATION_HIDDEN_ITEMS_CONFIG_KEY, ADMIN_NAVIGATION_ITEM_DEFINITIONS,
};
use crate::GatewayError;
use axum::{body::Bytes, http, response::IntoResponse, Json};
use serde_json::json;

impl<'a> AdminAppState<'a> {
    /// GET /api/admin/navigation/preferences
    ///
    /// 返回当前隐藏清单（归一化后）与全部可隐藏内置导航项目录，
    /// 供管理端“菜单管理”分区渲染开关列表。
    pub(crate) async fn build_admin_navigation_preferences_payload(
        &self,
    ) -> Result<serde_json::Value, GatewayError> {
        let stored = self
            .read_system_config_json_value(ADMIN_NAVIGATION_HIDDEN_ITEMS_CONFIG_KEY)
            .await?;
        Ok(json!({
            "hidden_items": stored_admin_navigation_hidden_items(stored.as_ref()),
            "items": admin_navigation_items_payload(),
        }))
    }

    /// PUT /api/admin/navigation/preferences
    ///
    /// 全量覆盖隐藏清单；空清单表示恢复全部内置项显示。
    /// key 校验失败返回 400，不落库。
    pub(crate) async fn apply_admin_navigation_preferences_update(
        &self,
        request_body: &Bytes,
    ) -> Result<Result<serde_json::Value, (http::StatusCode, serde_json::Value)>, GatewayError>
    {
        let parsed: serde_json::Value = match serde_json::from_slice(request_body) {
            Ok(value) => value,
            Err(_) => {
                return Ok(Err((
                    http::StatusCode::BAD_REQUEST,
                    json!({ "detail": "请求体格式错误，需要 hidden_items 字段" }),
                )));
            }
        };
        let hidden_items = match parse_admin_navigation_hidden_items(parsed.get("hidden_items")) {
            Ok(items) => items,
            Err((status, payload)) => return Ok(Err((status, payload))),
        };

        self.upsert_system_config_json_value(
            ADMIN_NAVIGATION_HIDDEN_ITEMS_CONFIG_KEY,
            &json!(hidden_items),
            Some("管理端内置导航菜单隐藏清单"),
        )
        .await?;

        Ok(Ok(json!({
            "hidden_items": hidden_items,
            "items": admin_navigation_items_payload(),
        })))
    }
}

pub(crate) fn admin_navigation_items_payload() -> Vec<serde_json::Value> {
    ADMIN_NAVIGATION_ITEM_DEFINITIONS
        .iter()
        .map(|item| {
            json!({
                "key": item.key,
                "href": item.href,
                "menu_group": item.menu_group,
                "display_name": item.display_name,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::ADMIN_NAVIGATION_HIDDEN_ITEMS_CONFIG_KEY;
    use crate::handlers::admin::request::AdminAppState;
    use crate::AppState;
    use axum::{body::Bytes, http::StatusCode};
    use serde_json::{json, Value};

    async fn read_stored_hidden_items(state: &AdminAppState<'_>) -> Value {
        state
            .read_system_config_json_value(ADMIN_NAVIGATION_HIDDEN_ITEMS_CONFIG_KEY)
            .await
            .expect("config read should succeed")
            .unwrap_or(Value::Null)
    }

    #[tokio::test]
    async fn get_preferences_defaults_to_empty_hidden_list_and_full_catalog() {
        let app = AppState::new().expect("app state should build");
        let state = AdminAppState::new(&app);

        let payload = state
            .build_admin_navigation_preferences_payload()
            .await
            .expect("payload should build");

        assert_eq!(payload["hidden_items"], json!([]));
        let items = payload["items"].as_array().expect("items should be array");
        assert_eq!(items.len(), 18);
        assert_eq!(items[0]["key"], json!("operations"));
        assert_eq!(items[0]["menu_group"], json!("overview"));
        assert_eq!(items[0]["href"], json!("/admin/operations"));
    }

    #[tokio::test]
    async fn put_preferences_persists_hidden_items_and_get_round_trips() {
        let app = AppState::new().expect("app state should build");
        let state = AdminAppState::new(&app);

        let body = Bytes::from_static(br#"{"hidden_items":["billingManagement","pool"]}"#);
        let response = state
            .apply_admin_navigation_preferences_update(&body)
            .await
            .expect("update should not fail internally")
            .expect("valid request should be accepted");
        assert_eq!(
            response["hidden_items"],
            json!(["billingManagement", "pool"])
        );

        let stored = read_stored_hidden_items(&state).await;
        assert_eq!(stored, json!(["billingManagement", "pool"]));

        let payload = state
            .build_admin_navigation_preferences_payload()
            .await
            .expect("payload should build");
        assert_eq!(
            payload["hidden_items"],
            json!(["billingManagement", "pool"])
        );
    }

    #[tokio::test]
    async fn put_preferences_with_empty_list_restores_all_items() {
        let app = AppState::new().expect("app state should build");
        let state = AdminAppState::new(&app);

        let hide_body = Bytes::from_static(br#"{"hidden_items":["usageRecords"]}"#);
        state
            .apply_admin_navigation_preferences_update(&hide_body)
            .await
            .expect("update should not fail internally")
            .expect("valid request should be accepted");

        let clear_body = Bytes::from_static(br#"{"hidden_items":[]}"#);
        let response = state
            .apply_admin_navigation_preferences_update(&clear_body)
            .await
            .expect("update should not fail internally")
            .expect("empty list is valid");
        assert_eq!(response["hidden_items"], json!([]));
        assert!(read_stored_hidden_items(&state)
            .await
            .as_array()
            .expect("stored value should be an array")
            .is_empty());
    }

    #[tokio::test]
    async fn put_preferences_rejects_unknown_keys_without_persisting() {
        let app = AppState::new().expect("app state should build");
        let state = AdminAppState::new(&app);

        let body = Bytes::from_static(br#"{"hidden_items":["not-a-nav-item"]}"#);
        let error = state
            .apply_admin_navigation_preferences_update(&body)
            .await
            .expect("update should not fail internally")
            .expect_err("unknown key must be rejected");
        assert_eq!(error.0, StatusCode::BAD_REQUEST);
        assert!(
            error.1["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("not-a-nav-item"),
            "error should mention the invalid key"
        );

        let stored = read_stored_hidden_items(&state).await;
        assert!(stored.is_null(), "rejected request must not persist config");
    }

    #[tokio::test]
    async fn put_preferences_rejects_malformed_bodies() {
        let app = AppState::new().expect("app state should build");
        let state = AdminAppState::new(&app);

        for body in [
            Bytes::from_static(br#"{"enabled":true}"#),
            Bytes::from_static(br#"{"hidden_items":"pool"}"#),
            Bytes::from_static(br#"not-json"#),
        ] {
            let error = state
                .apply_admin_navigation_preferences_update(&body)
                .await
                .expect("update should not fail internally")
                .expect_err("malformed body must be rejected");
            assert_eq!(error.0, StatusCode::BAD_REQUEST);
        }

        assert!(read_stored_hidden_items(&state).await.is_null());
    }
}
