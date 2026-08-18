use regex::Regex;
use serde_json::Value;

use super::{LocalFailoverPolicy, LocalFailoverRegexRule};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ParsedLocalErrorResponse {
    message: Option<String>,
    reason: Option<String>,
    raw: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LocalFailoverInput<'a> {
    pub(crate) status_code: u16,
    pub(crate) response_text: Option<&'a str>,
    pub(crate) failure_origin: FailureOrigin,
    pub(crate) replay_policy: OperationReplayPolicy,
}

impl<'a> LocalFailoverInput<'a> {
    #[cfg(test)]
    pub(crate) fn new(status_code: u16, response_text: Option<&'a str>) -> Self {
        Self::upstream_response(
            status_code,
            response_text,
            OperationReplayPolicy::Conservative,
        )
    }

    pub(crate) fn upstream_response(
        status_code: u16,
        response_text: Option<&'a str>,
        replay_policy: OperationReplayPolicy,
    ) -> Self {
        Self::trusted(
            status_code,
            response_text,
            failure_origin_from_upstream_response(status_code, response_text),
            replay_policy,
        )
    }

    pub(crate) fn trusted(
        status_code: u16,
        response_text: Option<&'a str>,
        failure_origin: FailureOrigin,
        replay_policy: OperationReplayPolicy,
    ) -> Self {
        Self {
            status_code,
            response_text: response_text
                .map(str::trim)
                .filter(|value| !value.is_empty()),
            failure_origin,
            replay_policy,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CallerFailureKind {
    ApiKey,
    Tenant,
    Semantic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureOrigin {
    Caller(CallerFailureKind),
    Request,
    UpstreamCredential,
    UpstreamProvider,
    Transport,
    Internal,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OperationReplayPolicy {
    Conservative,
    NoReplayAfterDispatch,
}

impl OperationReplayPolicy {
    pub(crate) const fn allows_candidate_switch(self) -> bool {
        matches!(self, Self::Conservative)
    }
}

/// Provider responses are trusted at this boundary. A 401 is an explicit
/// credential rejection; a 403 needs an authentication taxonomy because it
/// can also describe a non-retryable permission policy.
pub(crate) fn failure_origin_from_upstream_response(
    status_code: u16,
    response_text: Option<&str>,
) -> FailureOrigin {
    if status_code == 401 {
        return FailureOrigin::UpstreamCredential;
    }
    if status_code != 403 {
        return FailureOrigin::UpstreamProvider;
    }
    let is_authentication_error = serde_json::from_str::<Value>(response_text.unwrap_or_default())
        .ok()
        .and_then(|body| {
            body.get("error")
                .and_then(|error| error.get("type"))
                .or_else(|| body.get("type"))
                .and_then(Value::as_str)
                .map(str::trim)
                .map(|kind| kind.eq_ignore_ascii_case("authentication_error"))
        })
        .unwrap_or(false);
    if is_authentication_error {
        FailureOrigin::UpstreamCredential
    } else {
        FailureOrigin::UpstreamProvider
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalFailoverClassification {
    UseDefault,
    StopStatusCode,
    StopErrorPattern,
    StopExecutionError,
    StopCyberPolicy,
    StopFailureOrigin,
    StopReplayPolicy,
    RetrySuccessPattern,
    RetryStatusCode,
    RetryUpstreamFailure,
}

impl LocalFailoverClassification {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::UseDefault => "use_default",
            Self::StopStatusCode => "stop_status_code",
            Self::StopErrorPattern => "stop_error_pattern",
            Self::StopExecutionError => "stop_execution_error",
            Self::StopCyberPolicy => "stop_cyber_policy",
            Self::StopFailureOrigin => "stop_failure_origin",
            Self::StopReplayPolicy => "stop_replay_policy",
            Self::RetrySuccessPattern => "retry_success_pattern",
            Self::RetryStatusCode => "retry_status_code",
            Self::RetryUpstreamFailure => "retry_upstream_failure",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalTransportFailoverClassification {
    StopTransportError,
    RetryTransportError,
}

impl LocalTransportFailoverClassification {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::StopTransportError => "stop_transport_error",
            Self::RetryTransportError => "retry_transport_error",
        }
    }
}

pub(crate) const fn classify_local_transport_error(
    policy: &LocalFailoverPolicy,
    replay_policy: OperationReplayPolicy,
) -> LocalTransportFailoverClassification {
    if policy.stop_on_transport_errors || !replay_policy.allows_candidate_switch() {
        LocalTransportFailoverClassification::StopTransportError
    } else {
        LocalTransportFailoverClassification::RetryTransportError
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureRetryAction {
    Stop,
    SameCredential,
    NextCandidate,
    NextCredential,
    NextEndpoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureScope {
    None,
    Credential,
    CredentialModel,
    Endpoint,
    Provider,
}

impl FailureScope {
    pub(crate) const fn affects_credential(self) -> bool {
        matches!(self, Self::Credential | Self::CredentialModel)
    }

    pub(crate) const fn allows_key_wide_effects(self) -> bool {
        matches!(self, Self::None | Self::Credential)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureTokenAction {
    None,
    ForceRefresh,
    #[allow(dead_code)]
    Quarantine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FailureDisposition {
    pub(crate) retry_action: FailureRetryAction,
    pub(crate) failure_scope: FailureScope,
    pub(crate) token_action: FailureTokenAction,
    pub(crate) preserve_upstream_error: bool,
}

impl FailureDisposition {
    const fn new(
        retry_action: FailureRetryAction,
        failure_scope: FailureScope,
        token_action: FailureTokenAction,
        preserve_upstream_error: bool,
    ) -> Self {
        Self {
            retry_action,
            failure_scope,
            token_action,
            preserve_upstream_error,
        }
    }
}

pub(crate) const fn failure_disposition_from_local_classification(
    classification: LocalFailoverClassification,
    status_code: u16,
    failure_origin: FailureOrigin,
) -> FailureDisposition {
    match classification {
        LocalFailoverClassification::StopStatusCode
        | LocalFailoverClassification::StopErrorPattern
        | LocalFailoverClassification::StopExecutionError
        | LocalFailoverClassification::StopCyberPolicy
        | LocalFailoverClassification::StopFailureOrigin
        | LocalFailoverClassification::StopReplayPolicy => FailureDisposition::new(
            FailureRetryAction::Stop,
            FailureScope::None,
            FailureTokenAction::None,
            true,
        ),
        LocalFailoverClassification::UseDefault => FailureDisposition::new(
            FailureRetryAction::Stop,
            FailureScope::None,
            FailureTokenAction::None,
            status_code >= 400,
        ),
        LocalFailoverClassification::RetrySuccessPattern => {
            retry_disposition_for_origin(failure_origin, status_code, false)
        }
        LocalFailoverClassification::RetryStatusCode
        | LocalFailoverClassification::RetryUpstreamFailure => {
            retry_disposition_for_origin(failure_origin, status_code, false)
        }
    }
}

const fn retry_disposition_for_origin(
    failure_origin: FailureOrigin,
    status_code: u16,
    preserve_upstream_error: bool,
) -> FailureDisposition {
    match (failure_origin, status_code) {
        (FailureOrigin::UpstreamCredential, 401) => FailureDisposition::new(
            FailureRetryAction::NextCredential,
            FailureScope::Credential,
            FailureTokenAction::ForceRefresh,
            preserve_upstream_error,
        ),
        (FailureOrigin::UpstreamCredential, 403) => FailureDisposition::new(
            FailureRetryAction::NextCredential,
            FailureScope::Credential,
            FailureTokenAction::None,
            preserve_upstream_error,
        ),
        (FailureOrigin::UpstreamProvider, 529) => FailureDisposition::new(
            FailureRetryAction::NextEndpoint,
            FailureScope::Provider,
            FailureTokenAction::None,
            preserve_upstream_error,
        ),
        (FailureOrigin::UpstreamProvider, 500..=599) => FailureDisposition::new(
            FailureRetryAction::NextEndpoint,
            FailureScope::Endpoint,
            FailureTokenAction::None,
            preserve_upstream_error,
        ),
        (FailureOrigin::UpstreamProvider, _) => FailureDisposition::new(
            FailureRetryAction::NextCandidate,
            FailureScope::None,
            FailureTokenAction::None,
            preserve_upstream_error,
        ),
        _ => FailureDisposition::new(
            FailureRetryAction::Stop,
            FailureScope::None,
            FailureTokenAction::None,
            true,
        ),
    }
}

pub(crate) const fn classify_anthropic_failure_disposition(
    classification: LocalFailoverClassification,
    status_code: u16,
    failure_origin: FailureOrigin,
) -> FailureDisposition {
    if matches!(
        classification,
        LocalFailoverClassification::StopStatusCode
            | LocalFailoverClassification::StopErrorPattern
            | LocalFailoverClassification::StopExecutionError
            | LocalFailoverClassification::StopCyberPolicy
            | LocalFailoverClassification::StopFailureOrigin
            | LocalFailoverClassification::StopReplayPolicy
    ) {
        return failure_disposition_from_local_classification(
            classification,
            status_code,
            failure_origin,
        );
    }
    match classification {
        LocalFailoverClassification::RetryStatusCode
        | LocalFailoverClassification::RetryUpstreamFailure => {
            retry_disposition_for_origin(failure_origin, status_code, true)
        }
        _ => failure_disposition_from_local_classification(
            classification,
            status_code,
            failure_origin,
        ),
    }
}

pub(crate) fn classify_failure_disposition(
    provider_api_format: &str,
    classification: LocalFailoverClassification,
    status_code: u16,
    failure_origin: FailureOrigin,
) -> FailureDisposition {
    if provider_api_format
        .trim()
        .eq_ignore_ascii_case("claude:messages")
    {
        classify_anthropic_failure_disposition(classification, status_code, failure_origin)
    } else {
        failure_disposition_from_local_classification(classification, status_code, failure_origin)
    }
}

pub(crate) fn classify_local_failover(
    policy: &LocalFailoverPolicy,
    input: LocalFailoverInput<'_>,
) -> LocalFailoverClassification {
    if matches!(
        input.failure_origin,
        FailureOrigin::Caller(_)
            | FailureOrigin::Request
            | FailureOrigin::Internal
            | FailureOrigin::Unknown
            | FailureOrigin::Transport
    ) {
        return LocalFailoverClassification::StopFailureOrigin;
    }

    if input.failure_origin == FailureOrigin::UpstreamCredential {
        if !input.replay_policy.allows_candidate_switch() {
            return LocalFailoverClassification::StopReplayPolicy;
        }
        return if matches!(input.status_code, 401 | 403) {
            LocalFailoverClassification::RetryUpstreamFailure
        } else {
            LocalFailoverClassification::StopFailureOrigin
        };
    }

    if policy.stop_status_codes.contains(&input.status_code) {
        return LocalFailoverClassification::StopStatusCode;
    }

    if policy.stop_cyber_policy_errors
        && input.status_code >= 400
        && local_error_response_has_cyber_policy_code(input.response_text)
    {
        return LocalFailoverClassification::StopCyberPolicy;
    }

    if input.status_code >= 400
        && policy.error_stop_patterns.iter().any(|rule| {
            local_failover_regex_rule_matches(rule, input.response_text, input.status_code)
        })
    {
        return LocalFailoverClassification::StopErrorPattern;
    }

    if matches!(
        input.status_code,
        400 | 401 | 403 | 405 | 406 | 413 | 414 | 415 | 422
    ) && !policy.continue_status_codes.contains(&input.status_code)
    {
        return LocalFailoverClassification::StopStatusCode;
    }

    if input.status_code == 200
        && input.response_text.is_some_and(|text| {
            policy
                .success_failover_patterns
                .iter()
                .any(|rule| local_failover_regex_rule_matches(rule, Some(text), input.status_code))
        })
    {
        return retry_classification_for_replay_policy(
            input.replay_policy,
            LocalFailoverClassification::RetrySuccessPattern,
        );
    }

    if policy.continue_status_codes.contains(&input.status_code) {
        return retry_classification_for_replay_policy(
            input.replay_policy,
            LocalFailoverClassification::RetryStatusCode,
        );
    }

    if should_failover_local_upstream_status(
        input.status_code,
        policy.retry_client_errors_by_default,
    ) {
        return retry_classification_for_replay_policy(
            input.replay_policy,
            LocalFailoverClassification::RetryUpstreamFailure,
        );
    }

    LocalFailoverClassification::UseDefault
}

const fn retry_classification_for_replay_policy(
    replay_policy: OperationReplayPolicy,
    retry_classification: LocalFailoverClassification,
) -> LocalFailoverClassification {
    if replay_policy.allows_candidate_switch() {
        retry_classification
    } else {
        LocalFailoverClassification::StopReplayPolicy
    }
}

pub(crate) fn local_failover_error_message(response_text: Option<&str>) -> Option<String> {
    let parsed = parse_local_error_response(response_text);
    parsed
        .message
        .or(parsed.reason)
        .or(parsed.raw)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn should_failover_local_upstream_status(
    status_code: u16,
    retry_client_errors_by_default: bool,
) -> bool {
    status_code >= 500
        || matches!(status_code, 408 | 429)
        || ((400..500).contains(&status_code) && retry_client_errors_by_default)
}

fn local_error_response_has_cyber_policy_code(response_text: Option<&str>) -> bool {
    let Some(response_text) = response_text else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<Value>(response_text) else {
        return false;
    };
    json_value_has_cyber_policy_code(&value, 0)
}

fn json_value_has_cyber_policy_code(value: &Value, depth: usize) -> bool {
    if depth > 16 {
        return false;
    }
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            (key.eq_ignore_ascii_case("code") && value.as_str().is_some_and(is_cyber_policy_code))
                || json_value_has_cyber_policy_code(value, depth + 1)
        }),
        Value::Array(values) => values
            .iter()
            .any(|value| json_value_has_cyber_policy_code(value, depth + 1)),
        Value::String(text) => {
            let text = text.trim_start();
            if !text.starts_with('{') && !text.starts_with('[') {
                return false;
            }
            serde_json::from_str::<Value>(text)
                .ok()
                .is_some_and(|value| json_value_has_cyber_policy_code(&value, depth + 1))
        }
        _ => false,
    }
}

fn is_cyber_policy_code(code: &str) -> bool {
    let code = code.trim();
    code.eq_ignore_ascii_case("cyber_policy") || code.eq_ignore_ascii_case("cyber_policy_violation")
}

fn parse_local_error_response(response_text: Option<&str>) -> ParsedLocalErrorResponse {
    let raw = response_text
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let Some(raw_text) = raw.clone() else {
        return ParsedLocalErrorResponse::default();
    };

    let mut parsed = ParsedLocalErrorResponse {
        raw: Some(raw_text.clone()),
        ..ParsedLocalErrorResponse::default()
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw_text) else {
        parsed.message = Some(raw_text);
        return parsed;
    };

    let body_object = value.as_object();
    let error_object = body_object
        .and_then(|object| object.get("error"))
        .and_then(Value::as_object);

    parsed.message = first_non_empty_json_text(error_object, &["message", "detail", "reason"])
        .or_else(|| first_non_empty_json_text(body_object, &["errorMessage"]))
        .or_else(|| {
            body_object
                .and_then(|object| object.get("error"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
        .or_else(|| first_non_empty_json_text(body_object, &["message", "detail", "reason"]));
    parsed.reason = first_non_empty_json_text(error_object, &["reason", "code", "status"])
        .or_else(|| first_non_empty_json_text(body_object, &["reason", "code", "status"]));

    let Some(message) = parsed.message.clone() else {
        return parsed;
    };
    if !message.starts_with('{') {
        return parsed;
    }

    let Ok(nested) = serde_json::from_str::<Value>(&message) else {
        return parsed;
    };
    let nested_object = nested.as_object();
    let nested_error_object = nested_object
        .and_then(|object| object.get("error"))
        .and_then(Value::as_object);
    parsed.message =
        first_non_empty_json_text(nested_error_object, &["message", "detail", "reason"])
            .or_else(|| first_non_empty_json_text(nested_object, &["message", "detail", "reason"]))
            .or(parsed.message);
    parsed.reason = parsed
        .reason
        .or_else(|| first_non_empty_json_text(nested_error_object, &["reason", "code", "status"]))
        .or_else(|| first_non_empty_json_text(nested_object, &["reason", "code", "status"]));

    parsed
}

fn first_non_empty_json_text(
    object: Option<&serde_json::Map<String, Value>>,
    keys: &[&str],
) -> Option<String> {
    let object = object?;
    for key in keys {
        let Some(value) = object.get(*key) else {
            continue;
        };
        match value {
            Value::String(text) if !text.trim().is_empty() => return Some(text.trim().to_string()),
            Value::Number(number) => return Some(number.to_string()),
            _ => {}
        }
    }
    None
}

fn local_failover_regex_rule_matches(
    rule: &LocalFailoverRegexRule,
    response_text: Option<&str>,
    status_code: u16,
) -> bool {
    if !rule.status_codes.is_empty() && !rule.status_codes.contains(&status_code) {
        return false;
    }

    let pattern = rule.pattern.trim();
    if pattern.is_empty() {
        return !rule.status_codes.is_empty();
    }

    let Some(response_text) = response_text else {
        return false;
    };

    Regex::new(pattern)
        .ok()
        .is_some_and(|regex| regex.is_match(response_text))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{
        classify_anthropic_failure_disposition, classify_local_failover,
        classify_local_transport_error, failure_disposition_from_local_classification,
        failure_origin_from_upstream_response, CallerFailureKind, FailureDisposition,
        FailureOrigin, FailureRetryAction, FailureScope, FailureTokenAction,
        LocalFailoverClassification, LocalFailoverInput, LocalTransportFailoverClassification,
        OperationReplayPolicy,
    };
    use crate::orchestration::{LocalFailoverPolicy, LocalFailoverRegexRule};

    #[test]
    fn classifier_honors_explicit_stop_before_default_retryable_status() {
        let policy = LocalFailoverPolicy {
            stop_status_codes: [503].into_iter().collect(),
            ..LocalFailoverPolicy::default()
        };

        assert_eq!(
            classify_local_failover(&policy, LocalFailoverInput::new(503, None)),
            LocalFailoverClassification::StopStatusCode
        );
    }

    #[test]
    fn classifier_retries_transport_errors_by_default_and_honors_explicit_stop() {
        assert_eq!(
            classify_local_transport_error(
                &LocalFailoverPolicy::default(),
                OperationReplayPolicy::Conservative,
            ),
            LocalTransportFailoverClassification::RetryTransportError
        );

        let stop_policy = LocalFailoverPolicy {
            stop_on_transport_errors: true,
            ..LocalFailoverPolicy::default()
        };
        assert_eq!(
            classify_local_transport_error(&stop_policy, OperationReplayPolicy::Conservative),
            LocalTransportFailoverClassification::StopTransportError
        );
        assert_eq!(
            classify_local_transport_error(&stop_policy, OperationReplayPolicy::Conservative)
                .as_str(),
            "stop_transport_error"
        );
    }

    #[test]
    fn classifier_detects_success_failover_pattern() {
        let policy = LocalFailoverPolicy {
            success_failover_patterns: vec![LocalFailoverRegexRule {
                pattern: "relay:.*格式错误".to_string(),
                status_codes: BTreeSet::new(),
            }],
            ..LocalFailoverPolicy::default()
        };

        assert_eq!(
            classify_local_failover(
                &policy,
                LocalFailoverInput::new(200, Some("{\"error\":\"relay: 返回格式错误\"}"))
            ),
            LocalFailoverClassification::RetrySuccessPattern
        );
    }

    #[test]
    fn classifier_detects_error_stop_pattern() {
        let policy = LocalFailoverPolicy {
            error_stop_patterns: vec![LocalFailoverRegexRule {
                pattern: "content_policy_violation".to_string(),
                status_codes: [400, 403].into_iter().collect(),
            }],
            ..LocalFailoverPolicy::default()
        };

        assert_eq!(
            classify_local_failover(
                &policy,
                LocalFailoverInput::new(400, Some("{\"error\":\"content_policy_violation\"}"))
            ),
            LocalFailoverClassification::StopErrorPattern
        );
    }

    #[test]
    fn classifier_detects_error_stop_pattern_without_status_codes_on_any_error_status() {
        let policy = LocalFailoverPolicy {
            error_stop_patterns: vec![LocalFailoverRegexRule {
                pattern: "content_policy_violation".to_string(),
                status_codes: BTreeSet::new(),
            }],
            ..LocalFailoverPolicy::default()
        };

        for status_code in [400, 429, 503] {
            assert_eq!(
                classify_local_failover(
                    &policy,
                    LocalFailoverInput::new(
                        status_code,
                        Some("{\"error\":\"content_policy_violation\"}")
                    )
                ),
                LocalFailoverClassification::StopErrorPattern
            );
        }
    }

    #[test]
    fn classifier_detects_status_only_error_stop_rule_without_response_text() {
        let policy = LocalFailoverPolicy {
            error_stop_patterns: vec![LocalFailoverRegexRule {
                pattern: String::new(),
                status_codes: [429].into_iter().collect(),
            }],
            ..LocalFailoverPolicy::default()
        };

        assert_eq!(
            classify_local_failover(&policy, LocalFailoverInput::new(429, None)),
            LocalFailoverClassification::StopErrorPattern
        );
        assert_eq!(
            classify_local_failover(&policy, LocalFailoverInput::new(503, None)),
            LocalFailoverClassification::StopStatusCode
        );
    }

    #[test]
    fn classifier_stops_cyber_policy_when_policy_enabled() {
        let policy = LocalFailoverPolicy {
            stop_cyber_policy_errors: true,
            ..LocalFailoverPolicy::default()
        };

        assert_eq!(
            classify_local_failover(
                &policy,
                LocalFailoverInput::new(
                    400,
                    Some(
                        r#"{"type":"error","error":{"type":"invalid_request","code":"cyber_policy","message":"flagged"}}"#,
                    )
                )
            ),
            LocalFailoverClassification::StopCyberPolicy
        );
        assert_eq!(
            classify_local_failover(
                &policy,
                LocalFailoverInput::new(
                    400,
                    Some(r#"{"error":{"code":"cyber_policy_violation"}}"#)
                )
            ),
            LocalFailoverClassification::StopCyberPolicy
        );
        assert_eq!(
            classify_local_failover(
                &policy,
                LocalFailoverInput::new(
                    400,
                    Some(r#"{"outer":{"error":{"code":"cyber_policy"}}}"#)
                )
            ),
            LocalFailoverClassification::StopCyberPolicy
        );
        assert_eq!(
            classify_local_failover(
                &policy,
                LocalFailoverInput::new(400, Some(r#"{"error":{"code":"other"}}"#))
            ),
            LocalFailoverClassification::StopStatusCode
        );
    }

    #[test]
    fn classifier_retries_cyber_policy_when_policy_disabled() {
        let policy = LocalFailoverPolicy {
            stop_cyber_policy_errors: false,
            ..LocalFailoverPolicy::default()
        };
        assert_eq!(
            classify_local_failover(
                &policy,
                LocalFailoverInput::new(
                    400,
                    Some(r#"{"error":{"code":"cyber_policy","message":"flagged"}}"#)
                )
            ),
            LocalFailoverClassification::RetryUpstreamFailure
        );
    }

    #[test]
    fn classifier_detects_success_continue_status_code() {
        let policy = LocalFailoverPolicy {
            continue_status_codes: [200].into_iter().collect(),
            ..LocalFailoverPolicy::default()
        };

        assert_eq!(
            classify_local_failover(&policy, LocalFailoverInput::new(200, None)),
            LocalFailoverClassification::RetryStatusCode
        );
    }

    #[test]
    fn classifier_stops_default_request_errors_and_retries_transient_upstream_failures() {
        for (status_code, response_text) in [
            (
                400,
                "{\"error\":{\"type\":\"invalid_request_error\",\"message\":\"prompt is too long\"}}",
            ),
            (
                400,
                "{\"error\":{\"message\":\"Unsupported parameter: max_tokens is not supported with this model\"}}",
            ),
            (
                400,
                "{\"error\":{\"message\":\"Unknown parameter: 'tools[0].n'.\"}}",
            ),
            (
                400,
                "{\"error\":{\"message\":\"invalid model for this endpoint\"}}",
            ),
            (
                400,
                "{\"error\":{\"message\":\"invalid `signature` in `thinking` block: signature is for a different request\"}}",
            ),
            (
                400,
                "{\"error\":{\"message\":\"resource_exhausted: quota reached\"}}",
            ),
            (
                401,
                "{\"error\":{\"type\":\"invalid_request_error\",\"message\":\"Your authentication token has been invalidated. Please try signing in again.\"}}",
            ),
            (
                402,
                "{\"error\":{\"type\":\"invalid_request_error\",\"message\":\"payment required: credit balance exhausted\"}}",
            ),
            (
                403,
                "{\"error\":{\"type\":\"invalid_request_error\",\"message\":\"verify your account before continuing\"}}",
            ),
            (429, "{\"error\":{\"message\":\"rate limited\"}}"),
            (500, "{\"error\":{\"message\":\"upstream failed\"}}"),
        ] {
            let expected = if matches!(status_code, 429 | 500) {
                LocalFailoverClassification::RetryUpstreamFailure
            } else if status_code == 402 {
                LocalFailoverClassification::UseDefault
            } else {
                LocalFailoverClassification::StopStatusCode
            };
            assert_eq!(
                classify_local_failover(
                    &LocalFailoverPolicy::default(),
                    LocalFailoverInput::new(status_code, Some(response_text))
                ),
                expected
            );
        }
    }

    #[test]
    fn classifier_fail_closes_client_errors_independent_of_legacy_protocol_default() {
        let policy = LocalFailoverPolicy {
            retry_client_errors_by_default: false,
            ..LocalFailoverPolicy::default()
        };

        for status_code in [400, 401, 403] {
            assert_eq!(
                classify_local_failover(&policy, LocalFailoverInput::new(status_code, None)),
                LocalFailoverClassification::StopStatusCode
            );
        }
        assert_eq!(
            classify_local_failover(&policy, LocalFailoverInput::new(429, None)),
            LocalFailoverClassification::RetryUpstreamFailure
        );
        assert_eq!(
            classify_local_failover(&policy, LocalFailoverInput::new(499, None)),
            LocalFailoverClassification::UseDefault
        );
        assert_eq!(
            classify_local_failover(&policy, LocalFailoverInput::new(500, None)),
            LocalFailoverClassification::RetryUpstreamFailure
        );
    }

    #[test]
    fn classifier_explicit_continue_rule_overrides_protocol_client_error_default() {
        let policy = LocalFailoverPolicy {
            continue_status_codes: [429].into_iter().collect(),
            retry_client_errors_by_default: false,
            ..LocalFailoverPolicy::default()
        };

        assert_eq!(
            classify_local_failover(&policy, LocalFailoverInput::new(429, None)),
            LocalFailoverClassification::RetryStatusCode
        );
    }

    #[test]
    fn classifier_keeps_embedded_rate_limit_error_in_success_response_on_default_path() {
        assert_eq!(
            classify_local_failover(
                &LocalFailoverPolicy::default(),
                LocalFailoverInput::new(
                    200,
                    Some(
                        "{\"error\":{\"message\":\"quota reached\",\"type\":\"rate_limit_error\"}}"
                    )
                )
            ),
            LocalFailoverClassification::UseDefault
        );
    }

    #[test]
    fn legacy_classification_preserves_candidate_by_candidate_retry() {
        assert_eq!(
            failure_disposition_from_local_classification(
                LocalFailoverClassification::RetryUpstreamFailure,
                429,
                FailureOrigin::UpstreamProvider,
            ),
            FailureDisposition {
                retry_action: FailureRetryAction::NextCandidate,
                failure_scope: FailureScope::None,
                token_action: FailureTokenAction::None,
                preserve_upstream_error: false,
            }
        );
        assert_eq!(
            failure_disposition_from_local_classification(
                LocalFailoverClassification::StopErrorPattern,
                400,
                FailureOrigin::UpstreamProvider,
            )
            .retry_action,
            FailureRetryAction::Stop
        );
    }

    #[test]
    fn anthropic_bad_request_stops_and_preserves_upstream_error() {
        let disposition = classify_anthropic_failure_disposition(
            LocalFailoverClassification::StopStatusCode,
            400,
            FailureOrigin::UpstreamProvider,
        );

        assert_eq!(disposition.retry_action, FailureRetryAction::Stop);
        assert_eq!(disposition.failure_scope, FailureScope::None);
        assert_eq!(disposition.token_action, FailureTokenAction::None);
        assert!(disposition.preserve_upstream_error);
    }

    #[test]
    fn anthropic_auth_failures_refresh_then_rotate_only_when_needed() {
        let unauthorized = classify_anthropic_failure_disposition(
            LocalFailoverClassification::RetryUpstreamFailure,
            401,
            FailureOrigin::UpstreamCredential,
        );
        assert_eq!(
            unauthorized.retry_action,
            FailureRetryAction::NextCredential
        );
        assert_eq!(unauthorized.failure_scope, FailureScope::Credential);
        assert_eq!(unauthorized.token_action, FailureTokenAction::ForceRefresh);

        let forbidden = classify_anthropic_failure_disposition(
            LocalFailoverClassification::RetryUpstreamFailure,
            403,
            FailureOrigin::UpstreamCredential,
        );
        assert_eq!(forbidden.retry_action, FailureRetryAction::NextCredential);
        assert_eq!(forbidden.failure_scope, FailureScope::Credential);
        assert_eq!(forbidden.token_action, FailureTokenAction::None);
    }

    #[test]
    fn anthropic_rate_limit_does_not_guess_credential_origin() {
        let disposition = classify_anthropic_failure_disposition(
            LocalFailoverClassification::RetryUpstreamFailure,
            429,
            FailureOrigin::UpstreamProvider,
        );

        assert_eq!(disposition.retry_action, FailureRetryAction::NextCandidate);
        assert_eq!(disposition.failure_scope, FailureScope::None);
        assert!(!disposition.failure_scope.affects_credential());
        assert!(disposition.failure_scope.allows_key_wide_effects());
        assert!(disposition.preserve_upstream_error);
    }

    #[test]
    fn anthropic_overload_moves_endpoint_without_credential_penalty() {
        let disposition = classify_anthropic_failure_disposition(
            LocalFailoverClassification::RetryUpstreamFailure,
            529,
            FailureOrigin::UpstreamProvider,
        );

        assert_eq!(disposition.retry_action, FailureRetryAction::NextEndpoint);
        assert_eq!(disposition.failure_scope, FailureScope::Provider);
        assert!(!disposition.failure_scope.affects_credential());
        assert!(!disposition.failure_scope.allows_key_wide_effects());
        assert_eq!(disposition.token_action, FailureTokenAction::None);
        assert!(disposition.preserve_upstream_error);
    }

    #[test]
    fn anthropic_not_found_keeps_candidate_scope_and_oversize_stops() {
        let not_found = classify_anthropic_failure_disposition(
            LocalFailoverClassification::RetryUpstreamFailure,
            404,
            FailureOrigin::UpstreamProvider,
        );
        assert_eq!(not_found.retry_action, FailureRetryAction::NextCandidate);
        assert_eq!(not_found.failure_scope, FailureScope::None);
        assert!(not_found.preserve_upstream_error);

        let oversized = classify_anthropic_failure_disposition(
            LocalFailoverClassification::StopStatusCode,
            413,
            FailureOrigin::UpstreamProvider,
        );
        assert_eq!(oversized.retry_action, FailureRetryAction::Stop);
        assert_eq!(oversized.failure_scope, FailureScope::None);
        assert!(oversized.preserve_upstream_error);
    }

    #[test]
    fn only_unscoped_and_credential_failures_allow_key_wide_effects() {
        assert!(FailureScope::None.allows_key_wide_effects());
        assert!(FailureScope::Credential.allows_key_wide_effects());
        assert!(!FailureScope::CredentialModel.allows_key_wide_effects());
        assert!(!FailureScope::Endpoint.allows_key_wide_effects());
        assert!(!FailureScope::Provider.allows_key_wide_effects());
    }

    #[test]
    fn anthropic_explicit_stop_keeps_failure_resource_scope() {
        let auth = classify_anthropic_failure_disposition(
            LocalFailoverClassification::StopStatusCode,
            401,
            FailureOrigin::UpstreamCredential,
        );
        assert_eq!(auth.retry_action, FailureRetryAction::Stop);
        assert_eq!(auth.failure_scope, FailureScope::None);
        assert_eq!(auth.token_action, FailureTokenAction::None);

        let overloaded = classify_anthropic_failure_disposition(
            LocalFailoverClassification::StopStatusCode,
            529,
            FailureOrigin::UpstreamProvider,
        );
        assert_eq!(overloaded.retry_action, FailureRetryAction::Stop);
        assert_eq!(overloaded.failure_scope, FailureScope::None);
    }

    #[test]
    fn default_request_statuses_stop_without_body_inspection() {
        for status_code in [400, 405, 406, 413, 414, 415, 422] {
            assert_eq!(
                classify_local_failover(
                    &LocalFailoverPolicy::default(),
                    LocalFailoverInput::upstream_response(
                        status_code,
                        Some(r#"{"error":{"message":"retry me"}}"#),
                        OperationReplayPolicy::Conservative,
                    ),
                ),
                LocalFailoverClassification::StopStatusCode,
                "status {status_code} must stop by default"
            );
        }
    }

    #[test]
    fn unknown_request_internal_and_caller_origins_fail_closed() {
        for origin in [
            FailureOrigin::Unknown,
            FailureOrigin::Request,
            FailureOrigin::Internal,
            FailureOrigin::Caller(CallerFailureKind::ApiKey),
            FailureOrigin::Caller(CallerFailureKind::Tenant),
            FailureOrigin::Caller(CallerFailureKind::Semantic),
        ] {
            assert_eq!(
                classify_local_failover(
                    &LocalFailoverPolicy::default(),
                    LocalFailoverInput::trusted(
                        503,
                        Some(r#"{"error":{"message":"upstream-looking text"}}"#),
                        origin,
                        OperationReplayPolicy::Conservative,
                    ),
                ),
                LocalFailoverClassification::StopFailureOrigin,
                "origin {origin:?} must fail closed",
            );
        }
    }

    #[test]
    fn credential_rejection_rotates_only_when_replay_is_safe() {
        for status_code in [401, 403] {
            let credential_classification = classify_local_failover(
                &LocalFailoverPolicy::default(),
                LocalFailoverInput::trusted(
                    status_code,
                    Some(r#"{"error":{"message":"credential rejected"}}"#),
                    FailureOrigin::UpstreamCredential,
                    OperationReplayPolicy::Conservative,
                ),
            );
            assert_eq!(
                credential_classification,
                LocalFailoverClassification::RetryUpstreamFailure
            );
            assert_eq!(
                failure_disposition_from_local_classification(
                    credential_classification,
                    status_code,
                    FailureOrigin::UpstreamCredential,
                )
                .retry_action,
                FailureRetryAction::NextCredential
            );

            assert_eq!(
                classify_local_failover(
                    &LocalFailoverPolicy::default(),
                    LocalFailoverInput::trusted(
                        status_code,
                        Some(r#"{"error":{"message":"credential rejected"}}"#),
                        FailureOrigin::UpstreamCredential,
                        OperationReplayPolicy::NoReplayAfterDispatch,
                    ),
                ),
                LocalFailoverClassification::StopReplayPolicy,
                "credential rejection must not replay a dispatched non-idempotent request"
            );

            assert_eq!(
                classify_local_failover(
                    &LocalFailoverPolicy::default(),
                    LocalFailoverInput::upstream_response(
                        status_code,
                        Some(r#"{"error":{"message":"credential rejected"}}"#),
                        OperationReplayPolicy::Conservative,
                    ),
                ),
                LocalFailoverClassification::StopStatusCode
            );
        }
    }

    #[test]
    fn trusted_provider_auth_boundary_classifies_only_explicit_credentials() {
        assert_eq!(
            failure_origin_from_upstream_response(401, Some("not JSON")),
            FailureOrigin::UpstreamCredential
        );
        assert_eq!(
            failure_origin_from_upstream_response(
                403,
                Some(r#"{"type":"error","error":{"type":"authentication_error"}}"#),
            ),
            FailureOrigin::UpstreamCredential
        );
        assert_eq!(
            failure_origin_from_upstream_response(
                403,
                Some(r#"{"type":"error","error":{"type":"permission_error"}}"#),
            ),
            FailureOrigin::UpstreamProvider
        );
    }

    #[test]
    fn strict_replay_policy_stops_transient_and_success_envelope_replays() {
        let envelope_policy = LocalFailoverPolicy {
            success_failover_patterns: vec![LocalFailoverRegexRule {
                pattern: "quota".to_string(),
                status_codes: [200].into_iter().collect(),
            }],
            ..LocalFailoverPolicy::default()
        };

        for status_code in [408, 429, 500, 529] {
            assert_eq!(
                classify_local_failover(
                    &LocalFailoverPolicy::default(),
                    LocalFailoverInput::upstream_response(
                        status_code,
                        None,
                        OperationReplayPolicy::NoReplayAfterDispatch,
                    ),
                ),
                LocalFailoverClassification::StopReplayPolicy
            );
        }
        assert_eq!(
            classify_local_failover(
                &envelope_policy,
                LocalFailoverInput::upstream_response(
                    200,
                    Some(r#"{"error":{"message":"quota exceeded"}}"#),
                    OperationReplayPolicy::NoReplayAfterDispatch,
                ),
            ),
            LocalFailoverClassification::StopReplayPolicy
        );
        assert_eq!(
            classify_local_failover(
                &envelope_policy,
                LocalFailoverInput::upstream_response(
                    200,
                    Some(r#"{"error":{"message":"quota exceeded"}}"#),
                    OperationReplayPolicy::Conservative,
                ),
            ),
            LocalFailoverClassification::RetrySuccessPattern
        );
    }
}
