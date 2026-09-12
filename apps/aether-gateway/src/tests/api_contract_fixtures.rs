//! Executable contract checks for the public OpenAI/Claude API matrix.
//!
//! The fixture is intentionally kept under `docs/api/fixtures` so the same
//! rows are reviewable by API consumers and exercised by the gateway tests.

use crate::ai_serving::{build_core_error_body_for_client_format, LocalCoreSyncErrorKind};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;

pub(super) const FIXTURE: &str =
    include_str!("../../../../docs/api/fixtures/public-api-compatibility.json");

#[derive(Debug, Deserialize)]
struct FixtureDocument {
    version: u32,
    cases: Vec<CompatibilityCase>,
}

#[derive(Debug, Deserialize)]
struct CompatibilityCase {
    id: String,
    endpoint: String,
    client_format: String,
    request: Value,
    status: u16,
    envelope: Envelope,
    error_type: String,
    error_code: Option<String>,
    retry_after: Option<String>,
    retryable: bool,
    kind: ErrorKind,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Envelope {
    Openai,
    Claude,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ErrorKind {
    InvalidRequest,
    Authentication,
    PermissionDenied,
    NotFound,
    RequestTooLarge,
    RateLimit,
    QuotaExhausted,
    Overloaded,
    ServerError,
}

impl ErrorKind {
    fn to_local(&self) -> LocalCoreSyncErrorKind {
        match self {
            Self::InvalidRequest => LocalCoreSyncErrorKind::InvalidRequest,
            Self::Authentication => LocalCoreSyncErrorKind::Authentication,
            Self::PermissionDenied => LocalCoreSyncErrorKind::PermissionDenied,
            Self::NotFound => LocalCoreSyncErrorKind::NotFound,
            Self::RequestTooLarge => LocalCoreSyncErrorKind::RequestTooLarge,
            Self::RateLimit => LocalCoreSyncErrorKind::RateLimit,
            Self::QuotaExhausted => LocalCoreSyncErrorKind::QuotaExhausted,
            Self::Overloaded => LocalCoreSyncErrorKind::Overloaded,
            Self::ServerError => LocalCoreSyncErrorKind::ServerError,
        }
    }
}

#[test]
fn public_api_compatibility_fixture_matches_error_formatters_and_retry_policy() {
    let document: FixtureDocument =
        serde_json::from_str(FIXTURE).expect("public API compatibility fixture must be valid JSON");
    assert_eq!(document.version, 1, "unsupported fixture version");

    let mut ids = HashSet::new();
    let mut saw_openai_chat = false;
    let mut saw_openai_responses = false;
    let mut saw_openai_embeddings = false;
    let mut saw_openai_image_generations = false;
    let mut saw_openai_image_edits = false;
    let mut saw_claude = false;

    for case in document.cases {
        assert!(
            ids.insert(case.id.clone()),
            "duplicate fixture id: {}",
            case.id
        );
        assert!(
            !case.request.is_null(),
            "{} must include a request payload",
            case.id
        );
        assert!(
            (400..=599).contains(&case.status),
            "{} must describe an error status",
            case.id
        );
        assert!(
            matches!(
                (case.status, case.kind.to_local()),
                (400 | 405 | 422, LocalCoreSyncErrorKind::InvalidRequest)
                    | (401, LocalCoreSyncErrorKind::Authentication)
                    | (403, LocalCoreSyncErrorKind::PermissionDenied)
                    | (404, LocalCoreSyncErrorKind::NotFound)
                    | (413, LocalCoreSyncErrorKind::RequestTooLarge)
                    | (429, LocalCoreSyncErrorKind::RateLimit)
                    | (402 | 429, LocalCoreSyncErrorKind::QuotaExhausted)
                    | (503 | 529, LocalCoreSyncErrorKind::Overloaded)
                    | (500, LocalCoreSyncErrorKind::ServerError)
            ),
            "{} status and error kind disagree",
            case.id
        );
        if let Some(retry_after) = case.retry_after.as_deref() {
            assert!(
                matches!(case.status, 429 | 503 | 529),
                "{} Retry-After is only valid for retryable statuses",
                case.id
            );
            assert!(
                retry_after.parse::<u64>().is_ok_and(|seconds| seconds > 0),
                "{} Retry-After must be a positive integer",
                case.id
            );
            assert!(
                case.retryable,
                "{} Retry-After requires retryable=true",
                case.id
            );
        }

        match case.endpoint.as_str() {
            "/v1/chat/completions" => {
                assert_eq!(case.client_format, "openai:chat");
                saw_openai_chat = true;
            }
            "/v1/images/generations" => {
                assert_eq!(case.client_format, "openai:image");
                saw_openai_image_generations = true;
            }
            "/v1/responses" => {
                assert_eq!(case.client_format, "openai:responses");
                saw_openai_responses = true;
            }
            "/v1/embeddings" => {
                assert_eq!(case.client_format, "openai:embedding");
                saw_openai_embeddings = true;
            }
            "/v1/images/edits" => {
                assert_eq!(case.client_format, "openai:image");
                saw_openai_image_edits = true;
            }
            "/v1/messages" => {
                assert_eq!(case.client_format, "claude:messages");
                saw_claude = true;
            }
            endpoint => panic!("unsupported fixture endpoint {endpoint}"),
        }

        if case.kind.to_local() == LocalCoreSyncErrorKind::QuotaExhausted {
            assert_eq!(
                case.status,
                case.kind.to_local().http_status_code(&case.client_format)
            );
            assert!(
                !case.retryable,
                "insufficient_quota must not request a retry"
            );
            assert!(
                case.retry_after.is_none(),
                "insufficient_quota must omit Retry-After"
            );
        }

        let body = build_core_error_body_for_client_format(
            &case.client_format,
            "fixture error",
            case.error_code.as_deref(),
            case.kind.to_local(),
        )
        .expect("fixture client format must have an error envelope");

        match case.envelope {
            Envelope::Openai => {
                assert_eq!(body["error"]["type"], case.error_type, "{} type", case.id);
                assert_eq!(
                    body["error"].get("code").and_then(Value::as_str),
                    case.error_code.as_deref(),
                    "{} code",
                    case.id
                );
                assert!(
                    body.get("error").and_then(Value::as_object).is_some(),
                    "{} OpenAI envelope",
                    case.id
                );
            }
            Envelope::Claude => {
                assert_eq!(body["type"], "error", "{} top-level type", case.id);
                assert_eq!(body["error"]["type"], case.error_type, "{} type", case.id);
                assert_eq!(
                    body["error"].get("code").and_then(Value::as_str),
                    case.error_code.as_deref(),
                    "{} code",
                    case.id
                );
            }
        }
    }

    assert!(saw_openai_chat, "fixture must cover chat completions");
    assert!(saw_openai_responses, "fixture must cover Responses");
    assert!(saw_openai_embeddings, "fixture must cover Embeddings");
    assert!(
        saw_openai_image_generations,
        "fixture must cover image generations"
    );
    assert!(saw_openai_image_edits, "fixture must cover image edits");
    assert!(saw_claude, "fixture must cover Claude messages");
}
