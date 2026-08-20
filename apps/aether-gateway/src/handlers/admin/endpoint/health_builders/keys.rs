use crate::handlers::admin::request::AdminAppState;
use crate::provider_key_auth::provider_key_effective_api_formats;
use aether_scheduler_core::{
    count_recent_rpm_requests_for_provider_key_since,
    provider_key_circuit_payload_is_active_open_at,
};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

use aether_data_contracts::repository::provider_catalog::ProviderCatalogKeyHealthStateUpdate;

pub(crate) async fn build_admin_key_health_payload(
    state: &AdminAppState<'_>,
    key_id: &str,
    api_format: Option<&str>,
) -> Option<serde_json::Value> {
    if !state.has_provider_catalog_data_reader() {
        return None;
    }

    let key = state
        .read_provider_catalog_keys_by_ids(&[key_id.to_string()])
        .await
        .ok()
        .and_then(|mut keys| keys.drain(..).next())?;
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let provider = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&key.provider_id))
        .await
        .ok()
        .and_then(|mut providers| providers.drain(..).next())?;
    let endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&key.provider_id))
        .await
        .ok()
        .unwrap_or_default();

    let request_count = key.request_count.unwrap_or(0);
    let success_count = key.success_count.unwrap_or(0);
    let error_count = key
        .error_count
        .unwrap_or(request_count.saturating_sub(success_count));
    let avg_response_time_ms = match (key.total_response_time_ms, success_count) {
        (Some(total), successes) if successes > 0 => total as f64 / successes as f64,
        _ => 0.0,
    };

    let mut payload = json!({
        "key_id": key.id,
        "key_is_active": key.is_active,
        "key_statistics": {
            "request_count": request_count,
            "success_count": success_count,
            "error_count": error_count,
            "success_rate": if request_count > 0 {
                success_count as f64 / request_count as f64
            } else {
                0.0
            },
            "avg_response_time_ms": avg_response_time_ms,
        },
    });

    let health_by_format: Option<&serde_json::Map<String, serde_json::Value>> = key
        .health_by_format
        .as_ref()
        .and_then(serde_json::Value::as_object);
    let circuit_by_format: Option<&serde_json::Map<String, serde_json::Value>> = key
        .circuit_breaker_by_format
        .as_ref()
        .and_then(serde_json::Value::as_object);

    if let Some(api_format) = api_format.map(str::trim).filter(|value| !value.is_empty()) {
        let health_data = health_by_format.and_then(|formats| formats.get(api_format));
        let circuit_data = circuit_by_format.and_then(|formats| formats.get(api_format));

        payload["api_format"] = json!(api_format);
        payload["key_health_score"] = json!(health_data
            .and_then(|value| value.get("health_score"))
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(1.0));
        payload["key_consecutive_failures"] = json!(health_data
            .and_then(|value| value.get("consecutive_failures"))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0));
        payload["key_last_failure_at"] = health_data
            .and_then(|value| value.get("last_failure_at"))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        payload["circuit_breaker_open"] =
            json!(circuit_data.is_some_and(
                |value| provider_key_circuit_payload_is_active_open_at(value, now_unix_secs)
            ));
        payload["circuit_breaker_open_at"] = circuit_data
            .and_then(|value| value.get("open_at"))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        payload["next_probe_at"] = circuit_data
            .and_then(|value| value.get("next_probe_at"))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        payload["half_open_until"] = circuit_data
            .and_then(|value| value.get("half_open_until"))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        payload["half_open_successes"] = json!(circuit_data
            .and_then(|value| value.get("half_open_successes"))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0));
        payload["half_open_failures"] = json!(circuit_data
            .and_then(|value| value.get("half_open_failures"))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0));
    } else {
        let mut formats_payload = serde_json::Map::new();
        let mut any_circuit_open = false;
        for format_name in
            provider_key_effective_api_formats(&key, &provider.provider_type, &endpoints)
        {
            let health_data = health_by_format.and_then(|formats| formats.get(&format_name));
            let circuit_data = circuit_by_format.and_then(|formats| formats.get(&format_name));
            let active_open = circuit_data.is_some_and(|value| {
                provider_key_circuit_payload_is_active_open_at(value, now_unix_secs)
            });
            any_circuit_open |= active_open;
            formats_payload.insert(
                format_name.clone(),
                json!({
                    "health_score": health_data
                        .and_then(|value| value.get("health_score"))
                        .and_then(serde_json::Value::as_f64)
                        .unwrap_or(1.0),
                    "error_rate": 0.0,
                    "window_size": 0,
                    "consecutive_failures": health_data
                        .and_then(|value| value.get("consecutive_failures"))
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or(0),
                    "last_failure_at": health_data
                        .and_then(|value| value.get("last_failure_at"))
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                    "circuit_breaker": {
                        "state": if active_open { "open" } else { "closed" },
                        "open": active_open,
                        "open_at": circuit_data
                            .and_then(|value| value.get("open_at"))
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                        "next_probe_at": circuit_data
                            .and_then(|value| value.get("next_probe_at"))
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                        "half_open_until": circuit_data
                            .and_then(|value| value.get("half_open_until"))
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                        "half_open_successes": circuit_data
                            .and_then(|value| value.get("half_open_successes"))
                            .and_then(serde_json::Value::as_i64)
                            .unwrap_or(0),
                        "half_open_failures": circuit_data
                            .and_then(|value| value.get("half_open_failures"))
                            .and_then(serde_json::Value::as_i64)
                            .unwrap_or(0),
                    }
                }),
            );
        }

        let key_health_score = formats_payload
            .values()
            .filter_map(|value| value.get("health_score"))
            .filter_map(serde_json::Value::as_f64)
            .reduce(f64::min)
            .unwrap_or(1.0);

        payload["key_health_score"] = json!(key_health_score);
        payload["any_circuit_open"] = json!(any_circuit_open);
        payload["health_by_format"] = serde_json::Value::Object(formats_payload);
    }

    Some(payload)
}

pub(crate) async fn build_admin_key_rpm_payload(
    state: &AdminAppState<'_>,
    key_id: &str,
) -> Option<serde_json::Value> {
    if !state.has_provider_catalog_data_reader() {
        return None;
    }

    let key = state
        .read_provider_catalog_keys_by_ids(&[key_id.to_string()])
        .await
        .ok()
        .and_then(|mut keys| keys.drain(..).next())?;
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let recent_candidates = state.read_recent_request_candidates(256).await.ok()?;
    let reset_after_unix_secs = state.provider_key_rpm_reset_at(key.id.as_str(), now_unix_secs);
    let current_rpm = count_recent_rpm_requests_for_provider_key_since(
        &recent_candidates,
        key.id.as_str(),
        now_unix_secs,
        reset_after_unix_secs,
    );

    Some(json!({
        "key_id": key.id,
        "current_rpm": current_rpm,
        "rpm_limit": key.rpm_limit,
    }))
}

fn default_key_health_payload() -> serde_json::Value {
    json!({
        "health_score": 1.0,
        "consecutive_failures": 0,
        "last_failure_at": serde_json::Value::Null,
    })
}

fn default_key_circuit_payload() -> serde_json::Value {
    json!({
        "open": false,
        "open_at": serde_json::Value::Null,
        "next_probe_at": serde_json::Value::Null,
        "half_open_until": serde_json::Value::Null,
        "half_open_successes": 0,
        "half_open_failures": 0,
    })
}

fn reset_admin_circuit_entry(current: Option<&serde_json::Value>) -> serde_json::Value {
    let Some(circuit) = current.and_then(serde_json::Value::as_object) else {
        return default_key_circuit_payload();
    };
    if circuit.contains_key("half_open_claim")
        || circuit.contains_key("half_open_completion_pending")
    {
        return serde_json::Value::Object(circuit.clone());
    }
    let mut payload = default_key_circuit_payload();
    if let Some(fence) = circuit
        .get("half_open_fencing_token")
        .and_then(serde_json::Value::as_u64)
    {
        payload["half_open_fencing_token"] = json!(fence);
    }
    payload
}

fn repair_isolated_half_open_circuit_entry(
    current: &serde_json::Value,
    expected_fence: u64,
    expected_owner: &str,
) -> Result<serde_json::Value, &'static str> {
    let circuit = current.as_object().ok_or("circuit entry is malformed")?;
    if circuit.contains_key("half_open_completion_pending") {
        return Err("pending completion must be resolved before isolation repair");
    }
    if circuit
        .get("half_open_fencing_token")
        .and_then(serde_json::Value::as_u64)
        != Some(expected_fence)
    {
        return Err("durable fence does not match the operator expectation");
    }
    let claim = circuit
        .get("half_open_claim")
        .and_then(serde_json::Value::as_object)
        .ok_or("isolated half-open claim is absent or malformed")?;
    if claim.get("owner").and_then(serde_json::Value::as_str) != Some(expected_owner)
        || claim
            .get("fencing_token")
            .and_then(serde_json::Value::as_u64)
            != Some(expected_fence)
    {
        return Err("durable claim does not match the operator expectation");
    }
    if claim
        .get("expires_at_unix_ms")
        .and_then(serde_json::Value::as_u64)
        != Some(u64::MAX)
        || circuit
            .get("half_open_until_unix_ms")
            .and_then(serde_json::Value::as_u64)
            != Some(u64::MAX)
    {
        return Err("durable claim is not isolated");
    }

    let mut repaired = circuit.clone();
    repaired.remove("half_open_claim");
    repaired.insert(
        "half_open_until_unix_ms".to_string(),
        serde_json::Value::Null,
    );
    Ok(serde_json::Value::Object(repaired))
}

pub(crate) enum HalfOpenIsolationRepair {
    Repaired(serde_json::Value),
    NotFound,
    Rejected(String),
}

pub(crate) async fn repair_admin_half_open_isolation(
    state: &AdminAppState<'_>,
    key_id: &str,
    api_format: &str,
    expected_fence: u64,
    expected_owner: &str,
) -> Result<HalfOpenIsolationRepair, crate::GatewayError> {
    for _ in 0..4 {
        let Some(current) = state
            .read_provider_catalog_keys_by_ids(&[key_id.to_string()])
            .await?
            .into_iter()
            .next()
        else {
            return Ok(HalfOpenIsolationRepair::NotFound);
        };
        let mut circuits = current
            .circuit_breaker_by_format
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .cloned()
            .unwrap_or_default();
        let Some(current_circuit) = circuits.get(api_format) else {
            return Ok(HalfOpenIsolationRepair::NotFound);
        };
        let repaired = match repair_isolated_half_open_circuit_entry(
            current_circuit,
            expected_fence,
            expected_owner,
        ) {
            Ok(repaired) => repaired,
            Err(reason) => return Ok(HalfOpenIsolationRepair::Rejected(reason.to_string())),
        };
        let circuit_open = repaired
            .get("open")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        circuits.insert(api_format.to_string(), repaired);
        let update = ProviderCatalogKeyHealthStateUpdate {
            key_id: key_id.to_string(),
            expected_encrypted_auth_config: None,
            expected_health_by_format: current.health_by_format.clone(),
            expected_circuit_breaker_by_format: current.circuit_breaker_by_format.clone(),
            health_by_format: current.health_by_format,
            circuit_breaker_by_format: Some(serde_json::Value::Object(circuits)),
        };
        if state
            .compare_and_update_provider_catalog_key_health_state(&update)
            .await?
        {
            return Ok(HalfOpenIsolationRepair::Repaired(json!({
                "message": "Half-open 隔离已解除",
                "details": {
                    "key_id": key_id,
                    "api_format": api_format,
                    "repaired_fencing_token": expected_fence,
                    "repaired_owner": expected_owner,
                    "circuit_breaker_open": circuit_open,
                }
            })));
        }
        tokio::task::yield_now().await;
    }
    Ok(HalfOpenIsolationRepair::Rejected(
        "health state changed repeatedly during isolation repair".to_string(),
    ))
}

async fn recover_key_health_cas(
    state: &AdminAppState<'_>,
    key_id: &str,
    api_format: Option<&str>,
) -> Result<Option<bool>, crate::GatewayError> {
    for _ in 0..4 {
        let Some(current) = state
            .read_provider_catalog_keys_by_ids(&[key_id.to_string()])
            .await?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        let mut health = current
            .health_by_format
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .cloned()
            .unwrap_or_default();
        let current_circuits = current
            .circuit_breaker_by_format
            .as_ref()
            .and_then(serde_json::Value::as_object);
        let mut circuits = current_circuits.cloned().unwrap_or_default();
        if let Some(api_format) = api_format {
            health.insert(api_format.to_string(), default_key_health_payload());
            circuits.insert(
                api_format.to_string(),
                reset_admin_circuit_entry(current_circuits.and_then(|map| map.get(api_format))),
            );
        } else {
            health.clear();
            circuits = current_circuits
                .into_iter()
                .flat_map(|formats| formats.iter())
                .map(|(api_format, value)| {
                    (api_format.clone(), reset_admin_circuit_entry(Some(value)))
                })
                .collect();
        }
        let all_targeted_circuits_closed = if let Some(api_format) = api_format {
            circuits
                .get(api_format)
                .and_then(|circuit| circuit.get("open"))
                .and_then(serde_json::Value::as_bool)
                != Some(true)
        } else {
            circuits.values().all(|circuit| {
                circuit.get("open").and_then(serde_json::Value::as_bool) != Some(true)
            })
        };
        let update = ProviderCatalogKeyHealthStateUpdate {
            key_id: key_id.to_string(),
            expected_encrypted_auth_config: None,
            expected_health_by_format: current.health_by_format.clone(),
            expected_circuit_breaker_by_format: current.circuit_breaker_by_format.clone(),
            health_by_format: Some(serde_json::Value::Object(health)),
            circuit_breaker_by_format: Some(serde_json::Value::Object(circuits)),
        };
        if state
            .compare_and_update_provider_catalog_key_health_state(&update)
            .await?
        {
            return Ok(Some(all_targeted_circuits_closed));
        }
        tokio::task::yield_now().await;
    }
    Ok(None)
}

pub(crate) async fn recover_admin_key_health(
    state: &AdminAppState<'_>,
    key_id: &str,
    api_format: Option<&str>,
) -> Option<serde_json::Value> {
    let api_format = api_format.map(str::trim).filter(|value| !value.is_empty());
    let circuit_closed = recover_key_health_cas(state, key_id, api_format)
        .await
        .ok()??;

    let (message, details) = if let Some(api_format) = api_format {
        (
            format!("Key 的 {api_format} 格式已恢复"),
            json!({
                "api_format": api_format,
                "health_score": 1.0,
                "circuit_breaker_open": !circuit_closed,
            }),
        )
    } else {
        (
            "Key 所有格式已恢复".to_string(),
            json!({
                "health_score": 1.0,
                "circuit_breaker_open": !circuit_closed,
            }),
        )
    };

    Some(json!({
        "message": message,
        "details": details,
    }))
}

pub(crate) async fn recover_all_admin_key_health(
    state: &AdminAppState<'_>,
) -> Option<serde_json::Value> {
    if !state.has_provider_catalog_data_reader() {
        return None;
    }

    let providers = state
        .list_provider_catalog_providers(false)
        .await
        .ok()
        .unwrap_or_default();
    let provider_ids = providers
        .iter()
        .map(|provider| provider.id.clone())
        .collect::<Vec<_>>();
    let keys = if provider_ids.is_empty() {
        Vec::new()
    } else {
        state
            .list_provider_catalog_key_summaries_by_provider_ids(&provider_ids)
            .await
            .ok()
            .unwrap_or_default()
    };

    let recovered_keys = keys
        .into_iter()
        .filter(|key| {
            key.circuit_breaker_by_format
                .as_ref()
                .and_then(serde_json::Value::as_object)
                .map(|formats| {
                    formats.values().any(|circuit| {
                        circuit
                            .get("open")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();

    if recovered_keys.is_empty() {
        return Some(json!({
            "message": "没有需要恢复的 Key",
            "recovered_count": 0,
            "recovered_keys": [],
        }));
    }

    let mut payload_items = Vec::new();
    for key in recovered_keys {
        let Some(circuit_closed) = recover_key_health_cas(state, &key.id, None).await.ok()? else {
            continue;
        };
        let provider = state
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&key.provider_id))
            .await
            .ok()
            .and_then(|mut providers| providers.drain(..).next());
        let endpoints = state
            .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&key.provider_id))
            .await
            .ok()
            .unwrap_or_default();
        let api_formats = provider
            .as_ref()
            .map(|provider| {
                provider_key_effective_api_formats(&key, &provider.provider_type, &endpoints)
            })
            .unwrap_or_default();
        payload_items.push(json!({
            "key_id": key.id,
            "key_name": key.name,
            "provider_id": key.provider_id,
            "api_formats": api_formats,
            "circuit_breaker_open": !circuit_closed,
        }));
    }

    Some(json!({
        "message": format!("已恢复 {} 个 Key", payload_items.len()),
        "recovered_count": payload_items.len(),
        "recovered_keys": payload_items,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_recovery_preserves_fence_without_active_recovery() {
        let reset = reset_admin_circuit_entry(Some(&json!({
            "open": true,
            "half_open_fencing_token": 11
        })));
        assert_eq!(reset["open"], json!(false));
        assert_eq!(reset["half_open_fencing_token"], json!(11));
    }

    #[test]
    fn admin_recovery_preserves_active_claim_and_pending_completion_exactly() {
        let current = json!({
            "open": true,
            "half_open_fencing_token": 12,
            "half_open_claim": {"owner": "owner-a", "fencing_token": 12},
            "half_open_completion_pending": {"completion_id": "completion-a"}
        });
        assert_eq!(reset_admin_circuit_entry(Some(&current)), current);
    }

    #[test]
    fn operator_repair_clears_only_matching_isolation_and_keeps_circuit_open() {
        let current = json!({
            "open": true,
            "half_open_fencing_token": 12,
            "half_open_until_unix_ms": u64::MAX,
            "half_open_claim": {
                "owner": "owner-a",
                "fencing_token": 12,
                "expires_at_unix_ms": u64::MAX
            }
        });
        let repaired = repair_isolated_half_open_circuit_entry(&current, 12, "owner-a")
            .expect("matching isolated claim should repair");
        assert_eq!(repaired["open"], json!(true));
        assert_eq!(repaired["half_open_fencing_token"], json!(12));
        assert!(repaired.get("half_open_claim").is_none());
        assert!(repaired["half_open_until_unix_ms"].is_null());
    }

    #[test]
    fn operator_repair_rejects_stale_owner_fence_pending_and_non_isolated_claims() {
        let isolated = json!({
            "open": true,
            "half_open_fencing_token": 12,
            "half_open_until_unix_ms": u64::MAX,
            "half_open_claim": {
                "owner": "owner-a",
                "fencing_token": 12,
                "expires_at_unix_ms": u64::MAX
            }
        });
        assert!(repair_isolated_half_open_circuit_entry(&isolated, 11, "owner-a").is_err());
        assert!(repair_isolated_half_open_circuit_entry(&isolated, 12, "owner-b").is_err());

        let mut pending = isolated.clone();
        pending["half_open_completion_pending"] = json!({"completion_id": "pending"});
        assert!(repair_isolated_half_open_circuit_entry(&pending, 12, "owner-a").is_err());

        let mut active = isolated;
        active["half_open_claim"]["expires_at_unix_ms"] = json!(1000);
        active["half_open_until_unix_ms"] = json!(1000);
        assert!(repair_isolated_half_open_circuit_entry(&active, 12, "owner-a").is_err());
    }
}
