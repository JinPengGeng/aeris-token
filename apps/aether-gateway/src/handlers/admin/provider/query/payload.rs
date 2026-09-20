use axum::body::Bytes;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use std::collections::BTreeSet;

use aether_data_contracts::repository::emergency_chain::EmergencyChainTarget;

pub(crate) fn parse_admin_provider_query_body(
    request_body: Option<&Bytes>,
) -> Result<serde_json::Value, Response<axum::body::Body>> {
    let Some(raw_body) = request_body else {
        return Ok(json!({}));
    };
    if raw_body.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice::<serde_json::Value>(raw_body).map_err(|_| {
        super::response::build_admin_provider_query_bad_request_response(
            super::response::ADMIN_PROVIDER_QUERY_INVALID_JSON_DETAIL,
        )
    })
}

pub(crate) fn provider_query_extract_provider_id(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("provider_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn provider_query_extract_api_key_id(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("api_key_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn provider_query_insert_api_key_id(ids: &mut BTreeSet<String>, value: &str) {
    let value = value.trim();
    if !value.is_empty() {
        ids.insert(value.to_string());
    }
}

pub(crate) fn provider_query_extract_api_key_ids(
    payload: &serde_json::Value,
) -> Option<BTreeSet<String>> {
    let mut ids = BTreeSet::new();

    if let Some(value) = payload
        .get("api_key_ids")
        .or_else(|| payload.get("provider_key_ids"))
        .or_else(|| payload.get("key_ids"))
    {
        match value {
            serde_json::Value::Array(items) => {
                for item in items {
                    if let Some(value) = item.as_str() {
                        provider_query_insert_api_key_id(&mut ids, value);
                    }
                }
            }
            serde_json::Value::String(value) => {
                for item in value.split(',') {
                    provider_query_insert_api_key_id(&mut ids, item);
                }
            }
            _ => {}
        }
    }

    if let Some(api_key_id) = provider_query_extract_api_key_id(payload) {
        ids.insert(api_key_id);
    }

    (!ids.is_empty()).then_some(ids)
}

pub(crate) fn provider_query_extract_force_refresh(payload: &serde_json::Value) -> bool {
    payload
        .get("force_refresh")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

pub(crate) fn provider_query_extract_model(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("model")
        .or_else(|| payload.get("model_name"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn provider_query_extract_failover_models(payload: &serde_json::Value) -> Vec<String> {
    if let Some(items) = payload
        .get("failover_models")
        .or_else(|| payload.get("models"))
        .and_then(serde_json::Value::as_array)
    {
        return items
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
    }

    provider_query_extract_model(payload)
        .into_iter()
        .collect::<Vec<_>>()
}

pub(crate) fn provider_query_extract_request_id(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("request_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn provider_query_extract_emergency_targets(
    payload: &serde_json::Value,
    provider_id: &str,
) -> Option<Vec<EmergencyChainTarget>> {
    if !provider_query_emergency_identity_is_valid(provider_id) {
        return None;
    }
    let items = payload.get("targets")?.as_array()?;
    if items.is_empty() || items.len() > 32 {
        return None;
    }

    let mut identities = BTreeSet::new();
    let mut targets = Vec::with_capacity(items.len());
    for item in items {
        let endpoint_id = item
            .get("endpoint_id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        let key_id = item
            .get("key_id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        if !provider_query_emergency_identity_is_valid(endpoint_id)
            || !provider_query_emergency_identity_is_valid(key_id)
        {
            return None;
        }
        if !identities.insert((endpoint_id.to_string(), key_id.to_string())) {
            return None;
        }
        targets.push(EmergencyChainTarget {
            provider_id: provider_id.to_string(),
            endpoint_id: endpoint_id.to_string(),
            key_id: key_id.to_string(),
        });
    }
    Some(targets)
}

fn provider_query_emergency_identity_is_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_whitespace)
        && !value.chars().any(char::is_control)
        && !value.contains("://")
}

pub(crate) fn provider_query_payload_keys(payload: &serde_json::Value) -> Vec<String> {
    let Some(object) = payload.as_object() else {
        return Vec::new();
    };
    let mut keys = object.keys().cloned().collect::<Vec<_>>();
    keys.sort();
    keys
}

#[cfg(test)]
mod tests {
    use super::provider_query_extract_emergency_targets;
    use serde_json::json;

    #[test]
    fn emergency_targets_keep_request_order_and_reject_duplicates() {
        let targets = provider_query_extract_emergency_targets(
            &json!({
                "targets": [
                    { "endpoint_id": "endpoint-b", "key_id": "key-b" },
                    { "endpoint_id": "endpoint-a", "key_id": "key-a" }
                ]
            }),
            "provider-a",
        )
        .expect("ordered targets should parse");
        assert_eq!(targets[0].endpoint_id, "endpoint-b");
        assert_eq!(targets[1].endpoint_id, "endpoint-a");
        assert!(provider_query_extract_emergency_targets(
            &json!({
                "targets": [
                    { "endpoint_id": "endpoint-a", "key_id": "key-a" },
                    { "endpoint_id": "endpoint-a", "key_id": "key-a" }
                ]
            }),
            "provider-a",
        )
        .is_none());
    }
}
