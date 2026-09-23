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

fn default_sensitive_header_names() -> Vec<String> {
    DEFAULT_SENSITIVE_HEADER_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect()
}

/// Mutable holder for the sensitive header list. Production code uses the
/// process-wide [`registry`]; tests construct their own instance so parallel
/// test threads never mutate or observe each other's list.
#[derive(Debug)]
pub struct SensitiveHeaderRegistry {
    names: Vec<String>,
}

impl SensitiveHeaderRegistry {
    /// Registry seeded with [`DEFAULT_SENSITIVE_HEADER_NAMES`].
    pub fn with_default_names() -> Self {
        Self {
            names: default_sensitive_header_names(),
        }
    }

    pub fn names(&self) -> Vec<String> {
        self.names.clone()
    }

    pub fn set_names(&mut self, names: Vec<String>) {
        self.names = names;
    }

    /// Reset to [`DEFAULT_SENSITIVE_HEADER_NAMES`].
    pub fn reset_to_defaults(&mut self) {
        self.names = default_sensitive_header_names();
    }

    pub fn header_name_is_sensitive(&self, name: &str) -> bool {
        let trimmed = name.trim();
        !trimmed.is_empty()
            && self
                .names
                .iter()
                .any(|candidate| trimmed.eq_ignore_ascii_case(candidate))
    }

    /// Apply a raw system config value to this registry. Missing or malformed
    /// values reset to the defaults so an operator cannot disable masking
    /// entirely by accident.
    pub fn apply_config(&mut self, value: Option<&Value>) {
        match parse_sensitive_headers_config(value) {
            Some(names) => self.set_names(names),
            None => self.reset_to_defaults(),
        }
    }
}

impl Default for SensitiveHeaderRegistry {
    fn default() -> Self {
        Self::with_default_names()
    }
}

fn registry() -> &'static RwLock<SensitiveHeaderRegistry> {
    static REGISTRY: OnceLock<RwLock<SensitiveHeaderRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(SensitiveHeaderRegistry::with_default_names()))
}

pub fn sensitive_header_names() -> Vec<String> {
    registry()
        .read()
        .map(|guard| guard.names())
        .unwrap_or_else(|_| default_sensitive_header_names())
}

pub fn set_sensitive_header_names(names: Vec<String>) {
    if let Ok(mut guard) = registry().write() {
        guard.set_names(names);
    }
}

pub fn reset_sensitive_header_names_for_tests() {
    if let Ok(mut guard) = registry().write() {
        guard.reset_to_defaults();
    }
}

pub fn header_name_is_sensitive(name: &str) -> bool {
    registry()
        .read()
        .map(|guard| guard.header_name_is_sensitive(name))
        .unwrap_or_else(|_| {
            SensitiveHeaderRegistry::with_default_names().header_name_is_sensitive(name)
        })
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

/// Apply a raw system config value to the process-wide registry. Missing or
/// malformed values reset to the defaults so an operator cannot disable
/// masking entirely by accident.
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
        let registry = SensitiveHeaderRegistry::with_default_names();
        for name in [
            "Authorization",
            "X-API-KEY",
            "Api-Key",
            "X-Goog-Api-Key",
            "Cookie",
            "Set-Cookie",
            "Proxy-Authorization",
        ] {
            assert!(registry.header_name_is_sensitive(name), "{name}");
        }
        assert!(!registry.header_name_is_sensitive("x-request-id"));
        assert!(!registry.header_name_is_sensitive(""));
    }

    #[test]
    fn operator_config_extends_masked_headers() {
        // Own instance: parallel tests mutating the process-wide registry
        // cannot interleave with these assertions.
        let mut registry = SensitiveHeaderRegistry::with_default_names();
        registry.apply_config(Some(&json!(["x-custom-secret", "authorization"])));
        assert!(registry.header_name_is_sensitive("X-Custom-Secret"));
        assert!(registry.header_name_is_sensitive("authorization"));
        assert!(!registry.header_name_is_sensitive("x-goog-api-key"));
        registry.reset_to_defaults();
        assert!(registry.header_name_is_sensitive("x-goog-api-key"));
    }

    #[test]
    fn malformed_config_falls_back_to_defaults() {
        let mut registry = SensitiveHeaderRegistry::with_default_names();
        for value in [
            None,
            Some(json!([])),
            Some(json!("authorization")),
            Some(json!([1, 2])),
            Some(json!(["  "])),
        ] {
            registry.apply_config(value.as_ref());
            assert!(registry.header_name_is_sensitive("x-goog-api-key"));
        }
    }

    #[test]
    fn global_registry_delegates_to_shared_state() {
        // Single test that touches the process-wide registry; every other
        // test uses a local instance so no parallel thread races these
        // assertions.
        apply_sensitive_headers_config(Some(&json!(["x-global-check"])));
        assert!(header_name_is_sensitive("X-Global-Check"));
        reset_sensitive_header_names_for_tests();
        assert!(header_name_is_sensitive("x-goog-api-key"));
    }
}
