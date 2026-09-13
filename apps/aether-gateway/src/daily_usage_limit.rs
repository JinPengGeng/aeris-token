use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aether_cache::ExpiringMap;
use aether_data_contracts::repository::usage::DailyActualCostQuery;
use chrono::{DateTime, SecondsFormat, Utc};
use tracing::warn;

use crate::app_timezone::{app_timezone, local_day_window};
use crate::control::GatewayControlDecision;
use crate::stage_metrics::observe_gateway_stage_ms;
use crate::{AppState, GatewayError};

const SYSTEM_DAILY_USAGE_LIMIT_CONFIG_KEY: &str = "daily_usage_limit_usd";
const SYSTEM_CONFIG_CACHE_TTL: Duration = Duration::from_secs(15);
const LIMIT_EPSILON_USD: f64 = 0.000_000_01;
const USD_UNITS_PER_DOLLAR: f64 = 100_000_000.0;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DailyUsageScopeStatus {
    pub(crate) scope: &'static str,
    pub(crate) limit_usd: f64,
    pub(crate) used_usd: f64,
    pub(crate) remaining_usd: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FrontdoorDailyUsageStatus {
    pub(crate) available: bool,
    pub(crate) timezone: String,
    pub(crate) window_start: String,
    pub(crate) window_end: String,
    pub(crate) reset_at_unix_secs: u64,
    pub(crate) user: Option<DailyUsageScopeStatus>,
    pub(crate) key: Option<DailyUsageScopeStatus>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FrontdoorDailyUsageRejection {
    pub(crate) scope: &'static str,
    pub(crate) limit_usd: f64,
    pub(crate) used_usd: f64,
    pub(crate) remaining_usd: f64,
    pub(crate) retry_after: u64,
    pub(crate) reset_at_unix_secs: u64,
    pub(crate) timezone: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FrontdoorDailyUsageOutcome {
    NotApplicable,
    Allowed,
    Rejected(FrontdoorDailyUsageRejection),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DailyUsageLimitedResponse;

#[derive(Debug, Clone)]
pub(crate) struct FrontdoorDailyUsageLimiter {
    system_default_cache: Arc<ExpiringMap<String, f64>>,
    runtime_failures: Arc<AtomicU64>,
    #[cfg(test)]
    system_default_override: Arc<std::sync::Mutex<Option<f64>>>,
}

impl Default for FrontdoorDailyUsageLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl FrontdoorDailyUsageLimiter {
    pub(crate) fn new() -> Self {
        Self {
            system_default_cache: Arc::new(ExpiringMap::default()),
            runtime_failures: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            system_default_override: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    pub(crate) fn clear_system_default_cache(&self) {
        self.system_default_cache.clear();
    }

    pub(crate) fn runtime_failure_count(&self) -> u64 {
        self.runtime_failures.load(Ordering::Relaxed)
    }

    pub(crate) async fn check(
        &self,
        state: &AppState,
        decision: &GatewayControlDecision,
    ) -> FrontdoorDailyUsageOutcome {
        let started_at = Instant::now();
        let status_result = self.current_status(state, decision).await;
        observe_gateway_stage_ms(
            "daily_usage_limit_total",
            started_at.elapsed().as_millis() as u64,
        );
        let status = match status_result {
            Ok(Some(status)) => status,
            Ok(None) => return FrontdoorDailyUsageOutcome::NotApplicable,
            Err(err) => {
                aether_runtime::record_billing_fail_open_daily_quota();
                let failure_count = self.runtime_failures.fetch_add(1, Ordering::Relaxed) + 1;
                let auth = decision.auth_context.as_ref();
                warn!(
                    event_name = "frontdoor_daily_usage_check_failed",
                    log_type = "ops",
                    error = ?err,
                    runtime_failures_total = failure_count,
                    user_id = auth.map(|auth| auth.user_id.as_str()).unwrap_or("-"),
                    api_key_id = auth.map(|auth| auth.api_key_id.as_str()).unwrap_or("-"),
                    "daily usage limit check failed; allowing request"
                );
                return FrontdoorDailyUsageOutcome::Allowed;
            }
        };
        if !status.available {
            return FrontdoorDailyUsageOutcome::Allowed;
        }
        let exceeded = status
            .user
            .as_ref()
            .filter(|scope| scope.used_usd + LIMIT_EPSILON_USD >= scope.limit_usd)
            .or_else(|| {
                status
                    .key
                    .as_ref()
                    .filter(|scope| scope.used_usd + LIMIT_EPSILON_USD >= scope.limit_usd)
            });
        let Some(exceeded) = exceeded else {
            return FrontdoorDailyUsageOutcome::Allowed;
        };
        let now = Utc::now().timestamp().max(0) as u64;
        FrontdoorDailyUsageOutcome::Rejected(FrontdoorDailyUsageRejection {
            scope: exceeded.scope,
            limit_usd: exceeded.limit_usd,
            used_usd: exceeded.used_usd,
            remaining_usd: exceeded.remaining_usd,
            retry_after: status.reset_at_unix_secs.saturating_sub(now).max(1),
            reset_at_unix_secs: status.reset_at_unix_secs,
            timezone: status.timezone,
        })
    }

    pub(crate) async fn current_status(
        &self,
        state: &AppState,
        decision: &GatewayControlDecision,
    ) -> Result<Option<FrontdoorDailyUsageStatus>, GatewayError> {
        let Some(auth) = decision.auth_context.as_ref() else {
            return Ok(None);
        };
        if decision.route_class.as_deref() != Some("ai_public")
            || auth.local_rejection.is_some()
            || auth.user_id.is_empty()
            || auth.api_key_id.is_empty()
            || auth.admin_bypass_limits
            || auth.ip_bypass_limits
        {
            return Ok(None);
        }

        let needs_system_default = if auth.api_key_is_standalone {
            auth.api_key_daily_usage_limit_usd.is_none()
        } else {
            auth.user_daily_usage_limit_usd.is_none()
        };
        let system_limit = if needs_system_default {
            let config_started_at = Instant::now();
            let result = self.resolve_system_default_limit(state).await;
            observe_gateway_stage_ms(
                "daily_usage_limit_system_default",
                config_started_at.elapsed().as_millis() as u64,
            );
            result?
        } else {
            0.0
        };
        let (user_limit, key_limit) = resolve_scope_limits(
            auth.api_key_is_standalone,
            auth.user_daily_usage_limit_usd,
            auth.api_key_daily_usage_limit_usd,
            system_limit,
        );
        if user_limit.is_none() && key_limit.is_none() {
            return Ok(None);
        }

        let timezone = app_timezone();
        let now = Utc::now();
        let (_, start, end) = local_day_window(now, timezone);
        let read_started_at = Instant::now();
        let counts_result = state
            .data
            .read_daily_actual_cost_units(&DailyActualCostQuery {
                user_id: (!auth.api_key_is_standalone).then(|| auth.user_id.clone()),
                api_key_id: auth.api_key_id.clone(),
                start_unix_secs: start.timestamp().max(0) as u64,
                end_unix_secs: end.timestamp().max(0) as u64,
            })
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()));
        observe_gateway_stage_ms(
            "daily_usage_limit_persistent_read",
            read_started_at.elapsed().as_millis() as u64,
        );
        let counts = counts_result?;
        let user = user_limit
            .map(|limit_usd| scope_status("user", limit_usd, units_to_usd(counts.user_units)));
        let key = key_limit
            .map(|limit_usd| scope_status("key", limit_usd, units_to_usd(counts.key_units)));
        Ok(Some(FrontdoorDailyUsageStatus {
            available: true,
            timezone: timezone.name().to_string(),
            window_start: rfc3339(start),
            window_end: rfc3339(end),
            reset_at_unix_secs: end.timestamp().max(0) as u64,
            user,
            key,
        }))
    }

    async fn resolve_system_default_limit(&self, state: &AppState) -> Result<f64, GatewayError> {
        #[cfg(test)]
        if let Ok(guard) = self.system_default_override.lock() {
            if let Some(limit) = *guard {
                return Ok(limit);
            }
        }
        if let Some(limit) = self
            .system_default_cache
            .get_fresh(SYSTEM_DAILY_USAGE_LIMIT_CONFIG_KEY, SYSTEM_CONFIG_CACHE_TTL)
        {
            return Ok(limit);
        }
        let limit = parse_system_limit(
            state
                .read_system_config_json_value(SYSTEM_DAILY_USAGE_LIMIT_CONFIG_KEY)
                .await?,
        )?;
        self.system_default_cache.insert(
            SYSTEM_DAILY_USAGE_LIMIT_CONFIG_KEY.to_string(),
            limit,
            SYSTEM_CONFIG_CACHE_TTL,
            8,
        );
        Ok(limit)
    }

    #[cfg(test)]
    pub(crate) fn with_system_default_limit_for_tests(self, limit: f64) -> Self {
        if let Ok(mut guard) = self.system_default_override.lock() {
            *guard = Some(limit.max(0.0));
        }
        self
    }
}

fn scope_status(scope: &'static str, limit_usd: f64, used_usd: f64) -> DailyUsageScopeStatus {
    DailyUsageScopeStatus {
        scope,
        limit_usd,
        used_usd,
        remaining_usd: (limit_usd - used_usd).max(0.0),
    }
}

fn units_to_usd(value: u64) -> f64 {
    value as f64 / USD_UNITS_PER_DOLLAR
}

pub(crate) fn parse_system_limit(value: Option<serde_json::Value>) -> Result<f64, GatewayError> {
    let limit = match value {
        None | Some(serde_json::Value::Null) => 0.0,
        Some(serde_json::Value::Number(value)) => value.as_f64().ok_or_else(|| {
            GatewayError::Internal("invalid system config daily_usage_limit_usd".to_string())
        })?,
        Some(serde_json::Value::String(value)) => value.parse::<f64>().map_err(|_| {
            GatewayError::Internal("invalid system config daily_usage_limit_usd".to_string())
        })?,
        Some(_) => {
            return Err(GatewayError::Internal(
                "invalid system config daily_usage_limit_usd".to_string(),
            ))
        }
    };
    if !limit.is_finite() || limit < 0.0 {
        return Err(GatewayError::Internal(
            "invalid system config daily_usage_limit_usd".to_string(),
        ));
    }
    Ok(limit)
}

fn positive_limit(value: f64) -> Option<f64> {
    (value.is_finite() && value > 0.0).then_some(value)
}

fn resolve_scope_limits(
    is_standalone: bool,
    user_limit: Option<f64>,
    key_limit: Option<f64>,
    system_limit: f64,
) -> (Option<f64>, Option<f64>) {
    if is_standalone {
        (None, positive_limit(key_limit.unwrap_or(system_limit)))
    } else {
        (
            positive_limit(user_limit.unwrap_or(system_limit)),
            key_limit.and_then(positive_limit),
        )
    }
}

fn rfc3339(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::{
        parse_system_limit, positive_limit, resolve_scope_limits, FrontdoorDailyUsageLimiter,
        FrontdoorDailyUsageOutcome,
    };
    use crate::control::{GatewayControlAuthContext, GatewayControlDecision};
    use crate::data::GatewayDataState;
    use crate::AppState;
    use aether_data::repository::usage::InMemoryUsageReadRepository;
    use aether_data_contracts::repository::usage::StoredRequestUsageAudit;
    use aether_usage_runtime::UsageRecordWriter;
    use std::sync::Arc;

    fn sample_decision(user_limit: Option<f64>, key_limit: Option<f64>) -> GatewayControlDecision {
        let mut decision = GatewayControlDecision::synthetic(
            "/v1/chat/completions",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
        );
        decision.auth_context = Some(GatewayControlAuthContext {
            user_id: "user-1".to_string(),
            api_key_id: "key-1".to_string(),
            username: Some("alice".to_string()),
            api_key_name: Some("default".to_string()),
            api_key_billing_multiplier: 1.0,
            balance_remaining: None,
            access_allowed: true,
            user_rate_limit: None,
            api_key_rate_limit: None,
            user_daily_usage_limit_usd: user_limit,
            api_key_daily_usage_limit_usd: key_limit,
            api_key_is_standalone: false,
            admin_bypass_limits: false,
            ip_bypass_limits: false,
            local_rejection: None,
            allowed_models: None,
            ip_rules: None,
            verified_api_key_hash: None,
        });
        decision
    }

    fn state_with_usage(items: impl IntoIterator<Item = StoredRequestUsageAudit>) -> AppState {
        AppState::new()
            .expect("state should build")
            .with_usage_data_reader_for_tests(Arc::new(InMemoryUsageReadRepository::seed(items)))
    }

    fn state_with_daily_usage(actual_cost_usd: f64) -> AppState {
        state_with_usage([finalized_usage(
            "request-1",
            "user-1",
            "key-1",
            actual_cost_usd,
        )])
    }

    fn finalized_usage(
        request_id: &str,
        user_id: &str,
        api_key_id: &str,
        actual_cost_usd: f64,
    ) -> StoredRequestUsageAudit {
        let now = chrono::Utc::now();
        StoredRequestUsageAudit::new(
            format!("usage-{request_id}"),
            request_id.to_string(),
            Some(user_id.to_string()),
            Some(api_key_id.to_string()),
            None,
            None,
            "OpenAI".to_string(),
            "gpt-5".to_string(),
            None,
            None,
            None,
            None,
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            false,
            false,
            10,
            10,
            20,
            actual_cost_usd,
            actual_cost_usd,
            Some(200),
            None,
            None,
            Some(100),
            Some(20),
            "completed".to_string(),
            "settled".to_string(),
            now.timestamp_millis(),
            now.timestamp(),
            Some(now.timestamp()),
        )
        .expect("usage should build")
    }

    #[test]
    fn system_limit_defaults_to_unlimited_and_accepts_numbers_or_strings() {
        assert_eq!(parse_system_limit(None).unwrap(), 0.0);
        assert_eq!(
            parse_system_limit(Some(serde_json::Value::Null)).unwrap(),
            0.0
        );
        assert_eq!(
            parse_system_limit(Some(serde_json::json!(12.5))).unwrap(),
            12.5
        );
        assert_eq!(
            parse_system_limit(Some(serde_json::json!("8.25"))).unwrap(),
            8.25
        );
    }

    #[test]
    fn system_limit_rejects_invalid_or_negative_values() {
        for value in [
            serde_json::json!(-1),
            serde_json::json!("invalid"),
            serde_json::json!({ "limit": 1 }),
        ] {
            assert!(parse_system_limit(Some(value)).is_err());
        }
    }

    #[test]
    fn zero_is_unlimited_and_positive_values_are_limits() {
        assert_eq!(positive_limit(0.0), None);
        assert_eq!(positive_limit(-1.0), None);
        assert_eq!(positive_limit(f64::NAN), None);
        assert_eq!(positive_limit(0.01), Some(0.01));
    }

    #[test]
    fn normal_key_limit_only_adds_a_narrower_key_scope() {
        assert_eq!(
            resolve_scope_limits(false, None, None, 10.0),
            (Some(10.0), None)
        );
        assert_eq!(
            resolve_scope_limits(false, Some(20.0), Some(5.0), 10.0),
            (Some(20.0), Some(5.0))
        );
        assert_eq!(
            resolve_scope_limits(false, Some(20.0), Some(0.0), 10.0),
            (Some(20.0), None)
        );
    }

    #[test]
    fn standalone_key_inherits_or_explicitly_overrides_system_limit() {
        assert_eq!(
            resolve_scope_limits(true, None, None, 10.0),
            (None, Some(10.0))
        );
        assert_eq!(
            resolve_scope_limits(true, None, Some(0.0), 10.0),
            (None, None)
        );
        assert_eq!(
            resolve_scope_limits(true, None, Some(3.0), 10.0),
            (None, Some(3.0))
        );
    }

    #[tokio::test]
    async fn daily_usage_below_limit_is_allowed_and_at_limit_is_rejected() {
        let decision = sample_decision(Some(1.0), None);

        assert_eq!(
            FrontdoorDailyUsageLimiter::new()
                .check(&state_with_daily_usage(0.99), &decision)
                .await,
            FrontdoorDailyUsageOutcome::Allowed
        );
        match FrontdoorDailyUsageLimiter::new()
            .check(&state_with_daily_usage(1.0), &decision)
            .await
        {
            FrontdoorDailyUsageOutcome::Rejected(rejection) => {
                assert_eq!(rejection.scope, "user");
                assert_eq!(rejection.limit_usd, 1.0);
                assert_eq!(rejection.used_usd, 1.0);
                assert_eq!(rejection.remaining_usd, 0.0);
            }
            other => panic!("expected daily usage rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn usage_is_accumulated_before_a_limit_is_enabled() {
        let repository = Arc::new(InMemoryUsageReadRepository::default());
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(GatewayDataState::with_usage_repository_for_tests(
                repository,
            ));
        let record: aether_data_contracts::repository::usage::UpsertUsageRecord =
            serde_json::from_value(
                serde_json::to_value(finalized_usage(
                    "request-before-limit",
                    "user-1",
                    "key-1",
                    1.0,
                ))
                .unwrap(),
            )
            .unwrap();
        for _ in 0..3 {
            state
                .data
                .upsert_usage_record(record.clone())
                .await
                .expect("finalized usage replay should be accepted");
        }
        let limiter = FrontdoorDailyUsageLimiter::new();
        assert_eq!(
            limiter
                .check(&state, &sample_decision(Some(0.0), None))
                .await,
            FrontdoorDailyUsageOutcome::NotApplicable
        );

        match limiter
            .check(&state, &sample_decision(Some(1.0), None))
            .await
        {
            FrontdoorDailyUsageOutcome::Rejected(rejection) => {
                assert_eq!(rejection.scope, "user");
                assert_eq!(rejection.used_usd, 1.0);
            }
            other => panic!("expected accumulated usage rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn daily_usage_is_available_without_redis_recovery() {
        let state = state_with_daily_usage(1.25);
        let decision = sample_decision(Some(1.0), None);
        match FrontdoorDailyUsageLimiter::new()
            .check(&state, &decision)
            .await
        {
            FrontdoorDailyUsageOutcome::Rejected(rejection) => {
                assert_eq!(rejection.scope, "user");
                assert_eq!(rejection.used_usd, 1.25);
            }
            other => panic!("expected persistent daily usage rejection, got {other:?}"),
        }
        assert_eq!(
            state
                .runtime_state
                .kv_get("daily_usage_limit:runtime_state")
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn key_scope_can_narrow_a_normal_user_limit() {
        let limiter = FrontdoorDailyUsageLimiter::new();
        let decision = sample_decision(Some(10.0), Some(0.5));

        match limiter.check(&state_with_daily_usage(0.5), &decision).await {
            FrontdoorDailyUsageOutcome::Rejected(rejection) => {
                assert_eq!(rejection.scope, "key");
                assert_eq!(rejection.limit_usd, 0.5);
            }
            other => panic!("expected key daily usage rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn admin_and_ip_bypass_skip_daily_usage_repository_reads() {
        for field in ["admin", "ip"] {
            let limiter = FrontdoorDailyUsageLimiter::new();
            let mut decision = sample_decision(Some(1.0), None);
            let auth = decision.auth_context.as_mut().unwrap();
            auth.admin_bypass_limits = field == "admin";
            auth.ip_bypass_limits = field == "ip";

            assert_eq!(
                limiter
                    .check(&AppState::new().expect("state should build"), &decision)
                    .await,
                FrontdoorDailyUsageOutcome::NotApplicable
            );
            assert_eq!(limiter.runtime_failure_count(), 0);
        }
    }

    #[tokio::test]
    async fn unlimited_scopes_do_not_require_a_usage_repository() {
        let state = AppState::new().expect("state should build");
        let limiter = FrontdoorDailyUsageLimiter::new().with_system_default_limit_for_tests(0.0);
        for decision in [
            sample_decision(Some(0.0), Some(0.0)),
            sample_decision(None, None),
        ] {
            assert_eq!(
                limiter.check(&state, &decision).await,
                FrontdoorDailyUsageOutcome::NotApplicable
            );
        }
        assert_eq!(limiter.runtime_failure_count(), 0);
    }

    #[tokio::test]
    async fn missing_usage_repository_reports_an_error_and_checks_fail_open() {
        let state = AppState::new().expect("state should build");
        let limiter = FrontdoorDailyUsageLimiter::new();
        let decision = sample_decision(Some(1.0), None);

        let error = limiter.current_status(&state, &decision).await.unwrap_err();
        assert!(
            matches!(error, crate::error::GatewayError::Internal(message)
            if message.contains("daily usage limits require a usage reader"))
        );
        assert_eq!(
            limiter.check(&state, &decision).await,
            FrontdoorDailyUsageOutcome::Allowed
        );
        assert_eq!(limiter.runtime_failure_count(), 1);
    }

    #[tokio::test]
    async fn empty_persistent_usage_is_an_available_zero_balance() {
        let state = state_with_usage([]);
        let status = FrontdoorDailyUsageLimiter::new()
            .current_status(&state, &sample_decision(Some(1.0), Some(0.5)))
            .await
            .unwrap()
            .unwrap();
        assert!(status.available);
        assert_eq!(status.user.unwrap().used_usd, 0.0);
        assert_eq!(status.key.unwrap().used_usd, 0.0);
    }

    #[tokio::test]
    async fn current_day_usage_excludes_neighboring_days_and_other_scopes() {
        let (_, start, end) = crate::app_timezone::local_day_window(
            chrono::Utc::now(),
            crate::app_timezone::app_timezone(),
        );
        let mut previous = finalized_usage("previous-day", "user-1", "key-1", 3.0);
        previous.created_at_unix_ms = ((start.timestamp() - 1) * 1000) as u64;
        previous.updated_at_unix_secs = (start.timestamp() - 1) as u64;
        previous.finalized_at_unix_secs = Some((start.timestamp() - 1) as u64);
        let mut today = finalized_usage("today", "user-1", "key-1", 0.25);
        today.created_at_unix_ms = start.timestamp_millis() as u64;
        today.updated_at_unix_secs = start.timestamp() as u64;
        today.finalized_at_unix_secs = Some(start.timestamp() as u64);
        let mut next = finalized_usage("next-day", "user-1", "key-1", 4.0);
        next.created_at_unix_ms = end.timestamp_millis() as u64;
        next.updated_at_unix_secs = end.timestamp() as u64;
        next.finalized_at_unix_secs = Some(end.timestamp() as u64);
        let state = state_with_usage([
            previous,
            today,
            next,
            finalized_usage("other-key", "user-1", "key-2", 0.5),
            finalized_usage("other-user", "user-2", "key-3", 8.0),
        ]);
        let status = FrontdoorDailyUsageLimiter::new()
            .current_status(&state, &sample_decision(Some(1.0), Some(0.5)))
            .await
            .unwrap()
            .unwrap();

        assert!(status.available);
        assert_eq!(status.user.unwrap().used_usd, 0.75);
        assert_eq!(status.key.unwrap().used_usd, 0.25);
        assert_eq!(status.reset_at_unix_secs, end.timestamp() as u64);
    }

    #[tokio::test]
    async fn standalone_usage_only_applies_to_its_key_scope() {
        let mut standalone = finalized_usage("standalone", "user-1", "key-1", 0.5);
        standalone.request_metadata = Some(serde_json::json!({"api_key_is_standalone": true}));
        let state = state_with_usage([
            standalone,
            finalized_usage("normal", "user-1", "key-2", 0.25),
        ]);
        let limiter = FrontdoorDailyUsageLimiter::new();
        let mut decision = sample_decision(Some(0.1), Some(0.5));
        decision
            .auth_context
            .as_mut()
            .unwrap()
            .api_key_is_standalone = true;
        let status = limiter
            .current_status(&state, &decision)
            .await
            .unwrap()
            .unwrap();
        assert!(status.user.is_none());
        assert_eq!(status.key.unwrap().used_usd, 0.5);
        assert!(matches!(limiter.check(&state, &decision).await,
            FrontdoorDailyUsageOutcome::Rejected(rejection) if rejection.scope == "key"));

        let mut normal = sample_decision(Some(1.0), Some(0.5));
        normal.auth_context.as_mut().unwrap().api_key_id = "key-2".to_string();
        let status = limiter
            .current_status(&state, &normal)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status.user.unwrap().used_usd, 0.25);
        assert_eq!(status.key.unwrap().used_usd, 0.25);
    }

    #[tokio::test]
    async fn persistent_sum_error_is_not_reported_as_zero_usage() {
        let state = state_with_usage((0..3).map(|index| {
            finalized_usage(
                &format!("overflow-{index}"),
                "user-1",
                "key-1",
                70_000_000_000.0,
            )
        }));
        let limiter = FrontdoorDailyUsageLimiter::new();
        let decision = sample_decision(Some(1.0), None);
        assert!(limiter.current_status(&state, &decision).await.is_err());
        assert_eq!(
            limiter.check(&state, &decision).await,
            FrontdoorDailyUsageOutcome::Allowed
        );
        assert_eq!(limiter.runtime_failure_count(), 1);
    }
}
