use serde_json::{json, Value};

use super::LocalFailoverClassification;
use crate::handlers::shared::unix_secs_to_rfc3339;

const LOCAL_HEALTH_SCORE_FLOOR: f64 = 0.2;
pub(crate) const LOCAL_KEY_CIRCUIT_FAILURE_THRESHOLD: u64 = 8;
const LOCAL_KEY_CIRCUIT_MAX_PROBE_INTERVAL_MINUTES: u64 = 32;

pub(crate) fn project_local_failure_health(
    current_health_by_format: Option<&Value>,
    api_format: &str,
    classification: LocalFailoverClassification,
    status_code: u16,
    observed_at_unix_secs: u64,
) -> Option<Value> {
    if !local_candidate_failure_should_project_health(classification, status_code) {
        return None;
    }

    let api_format = api_format.trim();
    if api_format.is_empty() {
        return None;
    }

    let mut health_by_format = current_health_by_format
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let current = health_by_format
        .get(api_format)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let previous_failures = current
        .get("consecutive_failures")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .max(0) as u64;
    let consecutive_failures = previous_failures.saturating_add(1);

    health_by_format.insert(
        api_format.to_string(),
        json!({
            "health_score": projected_failure_health_score(classification, status_code, consecutive_failures),
            "consecutive_failures": consecutive_failures,
            "last_failure_at": unix_secs_to_rfc3339(observed_at_unix_secs),
        }),
    );

    Some(Value::Object(health_by_format))
}

pub(crate) fn project_local_success_health(
    current_health_by_format: Option<&Value>,
    api_format: &str,
) -> Option<Value> {
    let api_format = api_format.trim();
    if api_format.is_empty() {
        return None;
    }

    let mut health_by_format = current_health_by_format
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    health_by_format.insert(
        api_format.to_string(),
        json!({
            "health_score": 1.0,
            "consecutive_failures": 0,
            "last_failure_at": Value::Null,
        }),
    );
    Some(Value::Object(health_by_format))
}

pub(crate) fn project_local_key_circuit_open(
    current_circuit_by_format: Option<&Value>,
    api_format: &str,
    reason: &str,
    observed_at_unix_secs: u64,
    max_probe_interval_minutes: i32,
) -> Option<Value> {
    let api_format = api_format.trim();
    let reason = reason.trim();
    if api_format.is_empty() || reason.is_empty() {
        return None;
    }

    let mut circuit_by_format = current_circuit_by_format
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let current = circuit_by_format
        .get(api_format)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let max_probe_interval_minutes =
        normalize_max_probe_interval_minutes(max_probe_interval_minutes);
    let probe_interval_minutes =
        next_circuit_probe_interval_minutes(&current, max_probe_interval_minutes);
    let next_probe_at_unix_secs =
        next_probe_at_unix_secs(observed_at_unix_secs, probe_interval_minutes);
    let open_at = current
        .get("open_at")
        .filter(|_| current_bool(&current, "open"))
        .cloned()
        .unwrap_or_else(|| json!(unix_secs_to_rfc3339(observed_at_unix_secs)));
    let half_open_failures = current
        .get("half_open_failures")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .saturating_add(u64::from(current_bool(&current, "open")));
    let request_results_window =
        append_request_result_window(&current, observed_at_unix_secs, false);
    let half_open_fencing_token = current_u64(&current, "half_open_fencing_token");
    let half_open_claim = current.get("half_open_claim").cloned();
    let half_open_until_unix_ms = current.get("half_open_until_unix_ms").cloned();
    let half_open_completion_pending = current.get("half_open_completion_pending").cloned();
    circuit_by_format.insert(
        api_format.to_string(),
        json!({
            "open": true,
            "open_at": open_at,
            "reason": reason,
            "next_probe_at": unix_secs_to_rfc3339(next_probe_at_unix_secs),
            "next_probe_at_unix_secs": next_probe_at_unix_secs,
            "probe_interval_minutes": probe_interval_minutes,
            "max_probe_interval_minutes": max_probe_interval_minutes,
            "last_failure_at": unix_secs_to_rfc3339(observed_at_unix_secs),
            "last_probe_failure_at": if half_open_failures > 0 {
                json!(unix_secs_to_rfc3339(observed_at_unix_secs))
            } else {
                Value::Null
            },
            "half_open_until": Value::Null,
            "half_open_successes": 0,
            "half_open_failures": half_open_failures,
            "half_open_fencing_token": half_open_fencing_token,
            "half_open_claim": half_open_claim,
            "half_open_until_unix_ms": half_open_until_unix_ms,
            "half_open_completion_pending": half_open_completion_pending,
            "request_results_window": request_results_window,
        }),
    );
    Some(Value::Object(circuit_by_format))
}

pub(crate) fn project_local_key_circuit_failure(
    current_circuit_by_format: Option<&Value>,
    api_format: &str,
    observed_at_unix_secs: u64,
    consecutive_failures: u64,
    max_probe_interval_minutes: i32,
) -> Option<Value> {
    let api_format = api_format.trim();
    if api_format.is_empty() {
        return None;
    }

    let mut circuit_by_format = current_circuit_by_format
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let current = circuit_by_format
        .get(api_format)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let request_results_window =
        append_request_result_window(&current, observed_at_unix_secs, false);
    let already_open = current_bool(&current, "open");
    let half_open_fencing_token = current_u64(&current, "half_open_fencing_token");
    let half_open_claim = current.get("half_open_claim").cloned();
    let half_open_until_unix_ms = current.get("half_open_until_unix_ms").cloned();
    let half_open_completion_pending = current.get("half_open_completion_pending").cloned();
    if !already_open && consecutive_failures < LOCAL_KEY_CIRCUIT_FAILURE_THRESHOLD {
        circuit_by_format.insert(
            api_format.to_string(),
            json!({
                "open": false,
                "open_at": Value::Null,
                "reason": Value::Null,
                "next_probe_at": Value::Null,
                "next_probe_at_unix_secs": Value::Null,
                "probe_interval_minutes": 0,
                "max_probe_interval_minutes": normalize_max_probe_interval_minutes(max_probe_interval_minutes),
                "failure_count": consecutive_failures,
                "last_failure_at": unix_secs_to_rfc3339(observed_at_unix_secs),
                "last_probe_failure_at": Value::Null,
                "half_open_until": Value::Null,
                "half_open_successes": 0,
                "half_open_failures": 0,
                "half_open_fencing_token": half_open_fencing_token,
                "half_open_claim": half_open_claim,
                "half_open_until_unix_ms": half_open_until_unix_ms,
                "half_open_completion_pending": half_open_completion_pending,
                "request_results_window": request_results_window,
            }),
        );
        return Some(Value::Object(circuit_by_format));
    }

    let max_probe_interval_minutes =
        normalize_max_probe_interval_minutes(max_probe_interval_minutes);
    let probe_interval_minutes =
        next_circuit_probe_interval_minutes(&current, max_probe_interval_minutes);
    let next_probe_at_unix_secs =
        next_probe_at_unix_secs(observed_at_unix_secs, probe_interval_minutes);
    let open_at = current
        .get("open_at")
        .filter(|_| already_open)
        .cloned()
        .unwrap_or_else(|| json!(unix_secs_to_rfc3339(observed_at_unix_secs)));
    let half_open_failures = current
        .get("half_open_failures")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .saturating_add(u64::from(already_open));

    circuit_by_format.insert(
        api_format.to_string(),
        json!({
            "open": true,
            "open_at": open_at,
            "reason": format!("consecutive_failures_{LOCAL_KEY_CIRCUIT_FAILURE_THRESHOLD}"),
            "next_probe_at": unix_secs_to_rfc3339(next_probe_at_unix_secs),
            "next_probe_at_unix_secs": next_probe_at_unix_secs,
            "probe_interval_minutes": probe_interval_minutes,
            "max_probe_interval_minutes": max_probe_interval_minutes,
            "failure_count": consecutive_failures,
            "last_failure_at": unix_secs_to_rfc3339(observed_at_unix_secs),
            "last_probe_failure_at": if already_open {
                json!(unix_secs_to_rfc3339(observed_at_unix_secs))
            } else {
                Value::Null
            },
            "half_open_until": Value::Null,
            "half_open_successes": 0,
            "half_open_failures": half_open_failures,
            "half_open_fencing_token": half_open_fencing_token,
            "half_open_claim": half_open_claim,
            "half_open_until_unix_ms": half_open_until_unix_ms,
            "half_open_completion_pending": half_open_completion_pending,
            "request_results_window": request_results_window,
        }),
    );
    Some(Value::Object(circuit_by_format))
}

pub(crate) fn project_local_key_circuit_closed(
    current_circuit_by_format: Option<&Value>,
    api_format: &str,
) -> Option<Value> {
    let api_format = api_format.trim();
    if api_format.is_empty() {
        return None;
    }

    let mut circuit_by_format = current_circuit_by_format
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let half_open_fencing_token = circuit_by_format
        .get(api_format)
        .and_then(Value::as_object)
        .map(|current| current_u64(current, "half_open_fencing_token"))
        .unwrap_or(0);
    let half_open_claim = circuit_by_format
        .get(api_format)
        .and_then(Value::as_object)
        .and_then(|current| current.get("half_open_claim"))
        .cloned();
    let half_open_until_unix_ms = circuit_by_format
        .get(api_format)
        .and_then(Value::as_object)
        .and_then(|current| current.get("half_open_until_unix_ms"))
        .cloned();
    let half_open_completion_pending = circuit_by_format
        .get(api_format)
        .and_then(Value::as_object)
        .and_then(|current| current.get("half_open_completion_pending"))
        .cloned();
    circuit_by_format.insert(
        api_format.to_string(),
        json!({
            "open": false,
            "open_at": Value::Null,
            "reason": Value::Null,
            "next_probe_at": Value::Null,
            "next_probe_at_unix_secs": Value::Null,
            "half_open_until": Value::Null,
            "half_open_successes": 0,
            "half_open_failures": 0,
            "half_open_fencing_token": half_open_fencing_token,
            "half_open_claim": half_open_claim,
            "half_open_until_unix_ms": half_open_until_unix_ms,
            "half_open_completion_pending": half_open_completion_pending,
        }),
    );
    Some(Value::Object(circuit_by_format))
}

fn current_bool(current: &serde_json::Map<String, Value>, field: &str) -> bool {
    current.get(field).and_then(Value::as_bool).unwrap_or(false)
}

pub(crate) fn clear_local_half_open_claim(
    circuit_by_format: &mut Value,
    api_format: &str,
    expected_owner: &str,
    expected_fencing_token: u64,
) -> bool {
    let Some(circuit) = circuit_by_format
        .as_object_mut()
        .and_then(|circuits| circuits.get_mut(api_format))
        .and_then(Value::as_object_mut)
    else {
        return false;
    };
    if current_u64(circuit, "half_open_fencing_token") != expected_fencing_token {
        return false;
    }
    let claim_matches = circuit
        .get("half_open_claim")
        .and_then(Value::as_object)
        .is_some_and(|claim| {
            claim.get("owner").and_then(Value::as_str) == Some(expected_owner)
                && claim.get("fencing_token").and_then(Value::as_u64)
                    == Some(expected_fencing_token)
        });
    if !claim_matches {
        return false;
    }
    circuit.remove("half_open_claim");
    circuit.remove("half_open_until_unix_ms");
    true
}

pub(crate) fn reset_circuits_preserving_half_open_fences(
    current_circuit_by_format: Option<&Value>,
) -> Value {
    let mut reset = serde_json::Map::new();
    let Some(circuits) = current_circuit_by_format.and_then(Value::as_object) else {
        return Value::Object(reset);
    };
    for (api_format, value) in circuits {
        let Some(circuit) = value.as_object() else {
            continue;
        };
        let fence = current_u64(circuit, "half_open_fencing_token");
        let active_claim = circuit.get("half_open_claim").cloned();
        let pending = circuit.get("half_open_completion_pending").cloned();
        if active_claim.is_some() || pending.is_some() {
            reset.insert(api_format.clone(), value.clone());
        } else if fence > 0 {
            reset.insert(
                api_format.clone(),
                json!({
                    "open": false,
                    "open_at": Value::Null,
                    "reason": Value::Null,
                    "next_probe_at": Value::Null,
                    "next_probe_at_unix_secs": Value::Null,
                    "half_open_fencing_token": fence,
                }),
            );
        }
    }
    Value::Object(reset)
}

fn current_u64(current: &serde_json::Map<String, Value>, field: &str) -> u64 {
    current.get(field).and_then(Value::as_u64).unwrap_or(0)
}

fn normalize_max_probe_interval_minutes(value: i32) -> u64 {
    value.clamp(0, LOCAL_KEY_CIRCUIT_MAX_PROBE_INTERVAL_MINUTES as i32) as u64
}

fn next_circuit_probe_interval_minutes(
    current: &serde_json::Map<String, Value>,
    max_probe_interval_minutes: u64,
) -> u64 {
    if max_probe_interval_minutes == 0 {
        return 0;
    }
    if !current_bool(current, "open") {
        return 1.min(max_probe_interval_minutes);
    }
    current
        .get("probe_interval_minutes")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
        .saturating_mul(2)
        .min(max_probe_interval_minutes)
}

fn next_probe_at_unix_secs(observed_at_unix_secs: u64, interval_minutes: u64) -> u64 {
    observed_at_unix_secs.saturating_add(interval_minutes.saturating_mul(60))
}

fn append_request_result_window(
    current: &serde_json::Map<String, Value>,
    observed_at_unix_secs: u64,
    ok: bool,
) -> Value {
    let mut window = current
        .get("request_results_window")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    window.push(json!({
        "ts": observed_at_unix_secs,
        "ok": ok,
    }));
    let keep = usize::try_from(LOCAL_KEY_CIRCUIT_FAILURE_THRESHOLD)
        .unwrap_or(8)
        .max(1);
    if window.len() > keep {
        window = window.split_off(window.len() - keep);
    }
    Value::Array(window)
}

fn local_candidate_failure_should_project_health(
    classification: LocalFailoverClassification,
    status_code: u16,
) -> bool {
    if status_code < 400 {
        return false;
    }
    if status_code == 400 {
        return false;
    }

    match classification {
        LocalFailoverClassification::RetrySuccessPattern
        | LocalFailoverClassification::RetryStatusCode
        | LocalFailoverClassification::RetryUpstreamFailure => true,
        LocalFailoverClassification::UseDefault | LocalFailoverClassification::StopStatusCode => {
            status_code >= 500
        }
        LocalFailoverClassification::StopErrorPattern
        | LocalFailoverClassification::StopExecutionError
        | LocalFailoverClassification::StopCyberPolicy => false,
    }
}

fn projected_failure_health_score(
    classification: LocalFailoverClassification,
    status_code: u16,
    consecutive_failures: u64,
) -> f64 {
    let base_score = match classification {
        LocalFailoverClassification::RetrySuccessPattern => 0.75,
        _ if status_code >= 500 => 0.6,
        _ => 0.7,
    };

    let penalty = consecutive_failures.saturating_sub(1) as f64 * 0.15;
    let normalized = (base_score - penalty).max(LOCAL_HEALTH_SCORE_FLOOR);
    (normalized * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::{
        project_local_failure_health, project_local_key_circuit_closed,
        project_local_key_circuit_failure, project_local_key_circuit_open,
        project_local_success_health,
    };
    use crate::orchestration::LocalFailoverClassification;

    #[test]
    fn failure_projection_tracks_consecutive_failures_and_degrades_score() {
        let projected = project_local_failure_health(
            Some(&json!({
                "openai:chat": {
                    "health_score": 0.7,
                    "consecutive_failures": 1,
                    "last_failure_at": "2026-01-01T00:00:00+00:00"
                }
            })),
            "openai:chat",
            LocalFailoverClassification::RetryUpstreamFailure,
            503,
            1_760_000_000,
        )
        .expect("projection should exist");

        assert_eq!(projected["openai:chat"]["consecutive_failures"], json!(2));
        assert_eq!(projected["openai:chat"]["health_score"], json!(0.45));
        assert!(projected["openai:chat"]["last_failure_at"].is_string());
    }

    #[test]
    fn failure_projection_ignores_configured_stop_pattern() {
        assert!(project_local_failure_health(
            None,
            "openai:chat",
            LocalFailoverClassification::StopErrorPattern,
            400,
            1_760_000_000,
        )
        .is_none());
    }

    #[test]
    fn failure_projection_ignores_client_bad_request() {
        assert!(project_local_failure_health(
            None,
            "openai:chat",
            LocalFailoverClassification::RetryUpstreamFailure,
            400,
            1_760_000_000,
        )
        .is_none());
    }

    #[test]
    fn success_projection_resets_only_target_format() {
        let projected = project_local_success_health(
            Some(&json!({
                "openai:chat": {
                    "health_score": 0.4,
                    "consecutive_failures": 3,
                    "last_failure_at": "2026-01-01T00:00:00+00:00"
                },
                "openai:responses": {
                    "health_score": 0.8,
                    "consecutive_failures": 1,
                    "last_failure_at": "2026-01-02T00:00:00+00:00"
                }
            })),
            "openai:chat",
        )
        .expect("projection should exist");

        assert_eq!(
            projected["openai:chat"],
            json!({
                "health_score": 1.0,
                "consecutive_failures": 0,
                "last_failure_at": Value::Null,
            })
        );
        assert_eq!(projected["openai:responses"]["health_score"], json!(0.8));
    }

    #[test]
    fn circuit_open_projection_sets_probe_deadline() {
        let projected = project_local_key_circuit_open(
            None,
            "openai:chat",
            "account_deactivated_401",
            1_760_000_000,
            32,
        )
        .expect("projection should exist");

        assert_eq!(projected["openai:chat"]["open"], json!(true));
        assert_eq!(
            projected["openai:chat"]["reason"],
            json!("account_deactivated_401")
        );
        assert_eq!(
            projected["openai:chat"]["next_probe_at_unix_secs"],
            json!(1_760_000_060u64)
        );
        assert_eq!(projected["openai:chat"]["probe_interval_minutes"], json!(1));
    }

    #[test]
    fn consecutive_failure_circuit_opens_after_threshold_and_backs_off() {
        let before_threshold =
            project_local_key_circuit_failure(None, "openai:chat", 1_760_000_000, 7, 32)
                .expect("projection should exist");
        assert_eq!(before_threshold["openai:chat"]["open"], json!(false));

        let opened = project_local_key_circuit_failure(
            Some(&before_threshold),
            "openai:chat",
            1_760_000_060,
            8,
            32,
        )
        .expect("projection should exist");
        assert_eq!(opened["openai:chat"]["open"], json!(true));
        assert_eq!(
            opened["openai:chat"]["reason"],
            json!("consecutive_failures_8")
        );
        assert_eq!(opened["openai:chat"]["probe_interval_minutes"], json!(1));
        assert_eq!(
            opened["openai:chat"]["next_probe_at_unix_secs"],
            json!(1_760_000_120u64)
        );

        let backed_off =
            project_local_key_circuit_failure(Some(&opened), "openai:chat", 1_760_000_120, 9, 32)
                .expect("projection should exist");
        assert_eq!(
            backed_off["openai:chat"]["probe_interval_minutes"],
            json!(2)
        );
        assert_eq!(
            backed_off["openai:chat"]["next_probe_at_unix_secs"],
            json!(1_760_000_240u64)
        );
    }

    #[test]
    fn circuit_closed_projection_resets_format_circuit() {
        let projected = project_local_key_circuit_closed(
            Some(&json!({
                "openai:chat": {
                    "open": true,
                    "reason": "account_deactivated_401",
                    "next_probe_at_unix_secs": 1_760_001_920u64,
                    "half_open_fencing_token": 19,
                    "half_open_claim": {"owner": "node-a", "fencing_token": 19}
                }
            })),
            "openai:chat",
        )
        .expect("projection should exist");

        assert_eq!(projected["openai:chat"]["open"], json!(false));
        assert_eq!(projected["openai:chat"]["reason"], Value::Null);
        assert_eq!(
            projected["openai:chat"]["next_probe_at_unix_secs"],
            Value::Null
        );
        assert_eq!(
            projected["openai:chat"]["half_open_fencing_token"],
            json!(19)
        );
        assert_eq!(
            projected["openai:chat"]["half_open_claim"]["owner"],
            json!("node-a")
        );
        let mut completed = projected;
        assert!(super::clear_local_half_open_claim(
            &mut completed,
            "openai:chat",
            "node-a",
            19
        ));
        assert!(completed["openai:chat"].get("half_open_claim").is_none());
    }

    #[test]
    fn half_open_claim_clear_requires_exact_owner_and_claim_fence() {
        let original = json!({
            "openai:chat": {
                "half_open_fencing_token": 19,
                "half_open_until_unix_ms": 20_000,
                "half_open_claim": {
                    "owner": "owner-new",
                    "fencing_token": 19,
                    "expires_at_unix_ms": 20_000
                }
            }
        });

        let mut wrong_owner = original.clone();
        assert!(!super::clear_local_half_open_claim(
            &mut wrong_owner,
            "openai:chat",
            "owner-old",
            19
        ));
        assert_eq!(wrong_owner, original);

        let mut wrong_claim_fence = original.clone();
        wrong_claim_fence["openai:chat"]["half_open_claim"]["fencing_token"] = json!(18);
        let expected = wrong_claim_fence.clone();
        assert!(!super::clear_local_half_open_claim(
            &mut wrong_claim_fence,
            "openai:chat",
            "owner-new",
            19
        ));
        assert_eq!(wrong_claim_fence, expected);

        let mut exact = original;
        assert!(super::clear_local_half_open_claim(
            &mut exact,
            "openai:chat",
            "owner-new",
            19
        ));
        assert!(exact["openai:chat"].get("half_open_claim").is_none());
        assert!(exact["openai:chat"]
            .get("half_open_until_unix_ms")
            .is_none());
    }

    #[test]
    fn reset_preserves_authoritative_fence_and_active_recovery_records() {
        let reset = super::reset_circuits_preserving_half_open_fences(Some(&json!({
            "openai:chat": {
                "open": false,
                "half_open_fencing_token": 41
            },
            "claude:messages": {
                "open": true,
                "half_open_fencing_token": 42,
                "half_open_claim": {"owner": "node-a"},
                "half_open_completion_pending": {"completion_id": "completion-a"}
            },
            "gemini": {"open": false}
        })));

        assert_eq!(reset["openai:chat"]["half_open_fencing_token"], json!(41));
        assert_eq!(reset["openai:chat"]["open"], json!(false));
        assert_eq!(
            reset["claude:messages"]["half_open_claim"]["owner"],
            json!("node-a")
        );
        assert!(reset.get("gemini").is_none());
    }
}
