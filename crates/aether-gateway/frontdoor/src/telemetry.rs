//! Low-cardinality request telemetry policy shared by the HTTP front door.
//!
//! Request logs and RED metrics must never use raw paths, model names, user
//! identifiers, provider IDs, or error text as dimensions.  Keep the mapping
//! here deliberately small so a new route or provider cannot silently create
//! an unbounded label set.

use http::StatusCode;

/// Stable route classes emitted by the gateway control plane.
pub const ROUTE_CLASSES: &[&str] = &[
    "admin_proxy",
    "ai_public",
    "auth",
    "internal_proxy",
    "local",
    "passthrough",
    "public_support",
    "unknown",
];

/// Stable status classes used for request RED counters.
pub const STATUS_CLASSES: &[&str] = &["1xx", "2xx", "3xx", "4xx", "5xx", "unknown"];

/// Provider *types* are safe dimensions; provider IDs, model names and key
/// IDs are intentionally collapsed to `other`/`unknown`.
pub const PROVIDER_TYPES: &[&str] = &[
    "openai",
    "codex",
    "chatgpt_web",
    "claude_code",
    "kiro",
    "grok",
    "gemini_cli",
    "antigravity",
    "windsurf",
    "vertex_ai",
    "custom",
    "other",
    "unknown",
];

/// Internal response header populated by trusted execution paths.  The value
/// is still normalized before it reaches a log or metric label.
pub const PROVIDER_TYPE_HEADER: &str = "x-aether-telemetry-provider-type";

/// Map an arbitrary route class to the bounded contract set.
pub fn normalize_route_class(value: Option<&str>) -> &'static str {
    match value.map(str::trim) {
        Some("admin_proxy") => "admin_proxy",
        Some("ai_public") => "ai_public",
        Some("auth") => "auth",
        Some("internal_proxy") => "internal_proxy",
        Some("local") => "local",
        Some("passthrough") => "passthrough",
        Some("public_support") => "public_support",
        _ => "unknown",
    }
}

/// Convert an HTTP status into the fixed RED status class.
pub fn status_class(status: StatusCode) -> &'static str {
    match status.as_u16() {
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        500..=599 => "5xx",
        _ => "unknown",
    }
}

/// Keep provider telemetry to configured provider *types*, never IDs or names.
pub fn normalize_provider_type(value: Option<&str>) -> &'static str {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return "unknown";
    };
    let value = value.to_ascii_lowercase();
    match value.as_str() {
        "openai" => "openai",
        "codex" => "codex",
        "chatgpt_web" => "chatgpt_web",
        "claude_code" => "claude_code",
        "kiro" => "kiro",
        "grok" => "grok",
        "gemini_cli" => "gemini_cli",
        "antigravity" => "antigravity",
        "windsurf" => "windsurf",
        "vertex_ai" => "vertex_ai",
        "custom" => "custom",
        "other" => "other",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_provider_type, normalize_route_class, status_class};
    use http::StatusCode;

    #[test]
    fn route_classes_are_bounded() {
        assert_eq!(normalize_route_class(Some("ai_public")), "ai_public");
        assert_eq!(
            normalize_route_class(Some("new-route-with-user-id")),
            "unknown"
        );
        assert_eq!(normalize_route_class(None), "unknown");
    }

    #[test]
    fn status_classes_cover_http_ranges() {
        assert_eq!(status_class(StatusCode::CONTINUE), "1xx");
        assert_eq!(status_class(StatusCode::OK), "2xx");
        assert_eq!(status_class(StatusCode::FOUND), "3xx");
        assert_eq!(status_class(StatusCode::BAD_REQUEST), "4xx");
        assert_eq!(status_class(StatusCode::BAD_GATEWAY), "5xx");
    }

    #[test]
    fn provider_labels_collapse_ids_and_unknown_values() {
        assert_eq!(normalize_provider_type(Some("OpenAI")), "openai");
        assert_eq!(
            normalize_provider_type(Some("provider-secret-id")),
            "unknown"
        );
        assert_eq!(normalize_provider_type(Some("")), "unknown");
    }
}
