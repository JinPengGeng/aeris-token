//! Operator-configurable sensitive header names for trace/log masking.
//!
//! Gateway trace masking historically applied a fixed header list. The admin
//! `sensitive_headers` system config now feeds this registry so custom header
//! names are masked too. Missing or malformed config keeps the defaults.

use serde_json::Value;
use std::sync::{OnceLock, RwLock};

/// Admin system config key consumed by gateway trace masking.
pub const SENSITIVE_HEADERS_SYSTEM_CONFIG_KEY: &str = "sensitive_headers";

/// Default list preserved from the original hardcoded gateway masking. It is
/// intentionally a superset of the admin default (which additionally lacks
/// `x-goog-api-key` and `proxy-authorization`).
pub const DEFAULT_SENSITIVE_HEADER_NAMES: &[&str] = &[
    "authorization",
    "x-api-key",
    "api-key",
    "x-goog-api-key",
    "cookie",
    "set-cookie",
    "proxy-authorization",
];

fn registry() -> &'static RwLock<Vec<String>> {
    static REGISTRY: OnceLock<RwLock<Vec<String>>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(default_sensitive_header_names()))
}

fn default_sensitive_header_names() -> Vec<String> {
    DEFAULT_SENSITIVE_HEADER_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect()
}

pub fn sensitive_header_names() -> Vec<String> {
    registry()
        .read()
        .map(|guard| guard.clone())
        .unwrap_or_else(|_| default_sensitive_header_names())
}

pub fn set_sensitive_header_names(names: Vec<String>) {
    if let Ok(mut guard) = registry().write() {
        *guard = names;
    }
}

pub fn reset_sensitive_header_names_for_tests() {
    set_sensitive_header_names(default_sensitive_header_names());
}

pub fn header_name_is_sensitive(name: &str) -> bool {
    let trimmed = name.trim();
    !trimmed.is_empty()
        && sensitive_header_names()
            .iter()
            .any(|candidate| trimmed.eq_ignore_ascii_case(candidate))
}

/// Parse the admin system config value. Returns `None` when the value is
/// missing or malformed so callers keep the default behavior.
pub fn parse_sensitive_headers_config(value: Option<&Value>) -> Option<Vec<String>> {
    let names = value?
        .as_array()?
        .iter()
        .filter_map(|entry| {
            entry
                .as_str()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    if names.is_empty() {
        None
    } else {
        Some(names)
    }
}

/// Apply a raw system config value to the registry. Missing or malformed
/// values reset to the defaults so an operator cannot disable masking
/// entirely by accident.
pub fn apply_sensitive_headers_config(value: Option<&Value>) {
    match parse_sensitive_headers_config(value) {
        Some(names) => set_sensitive_header_names(names),
        None => reset_sensitive_header_names_for_tests(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_mask_the_historical_gateway_list() {
        reset_sensitive_header_names_for_tests();
        for name in [
            "Authorization",
            "X-API-KEY",
            "Api-Key",
            "X-Goog-Api-Key",
            "Cookie",
            "Set-Cookie",
            "Proxy-Authorization",
        ] {
            assert!(header_name_is_sensitive(name), "{name}");
        }
        assert!(!header_name_is_sensitive("x-request-id"));
        assert!(!header_name_is_sensitive(""));
    }

    #[test]
    fn operator_config_extends_masked_headers() {
        apply_sensitive_headers_config(Some(&json!(["x-custom-secret", "authorization"])));
        assert!(header_name_is_sensitive("X-Custom-Secret"));
        assert!(header_name_is_sensitive("authorization"));
        assert!(!header_name_is_sensitive("x-goog-api-key"));
        reset_sensitive_header_names_for_tests();
        assert!(header_name_is_sensitive("x-goog-api-key"));
    }

    #[test]
    fn malformed_config_falls_back_to_defaults() {
        for value in [
            None,
            Some(json!([])),
            Some(json!("authorization")),
            Some(json!([1, 2])),
            Some(json!(["  "])),
        ] {
            apply_sensitive_headers_config(value.as_ref());
            assert!(header_name_is_sensitive("x-goog-api-key"));
        }
        reset_sensitive_header_names_for_tests();
    }
}
