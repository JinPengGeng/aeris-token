use crate::core::{OAuthAuthorizeResponse, OAuthError, OAuthTokenSet};
use crate::network::{OAuthHttpExecutor, OAuthNetworkContext};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, PartialEq)]
/// Data type: identity oauth provider config.
pub struct IdentityOAuthProviderConfig {
    /// Field: provider type.
    pub provider_type: String,
    /// Field: display name.
    pub display_name: String,
    /// Field: authorization url.
    pub authorization_url: String,
    /// Field: token url.
    pub token_url: String,
    /// Field: userinfo url.
    pub userinfo_url: Option<String>,
    /// Field: client id.
    pub client_id: String,
    /// Field: client secret.
    pub client_secret: Option<String>,
    /// Field: scopes.
    pub scopes: Vec<String>,
    /// Field: redirect uri.
    pub redirect_uri: String,
    /// Field: frontend callback url.
    pub frontend_callback_url: String,
    /// Field: attribute mapping.
    pub attribute_mapping: Option<Value>,
    /// Field: extra config.
    pub extra_config: Option<Value>,
}

impl std::fmt::Debug for IdentityOAuthProviderConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IdentityOAuthProviderConfig")
            .field("provider_type", &self.provider_type)
            .field("display_name", &self.display_name)
            .field("authorization_url", &"[REDACTED]")
            .field("token_url", &"[REDACTED]")
            .field(
                "userinfo_url",
                &self.userinfo_url.as_ref().map(|_| "[REDACTED]"),
            )
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "[REDACTED]"),
            )
            .field("scopes", &self.scopes)
            .field("redirect_uri", &self.redirect_uri)
            .field("frontend_callback_url", &self.frontend_callback_url)
            .field(
                "attribute_mapping",
                &self.attribute_mapping.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "extra_config",
                &self.extra_config.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

#[derive(Clone, PartialEq)]
/// Data type: identity oauth start context.
pub struct IdentityOAuthStartContext {
    /// Field: state.
    pub state: String,
    /// Field: code challenge.
    pub code_challenge: Option<String>,
    /// Field: network.
    pub network: OAuthNetworkContext,
}

impl std::fmt::Debug for IdentityOAuthStartContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IdentityOAuthStartContext")
            .field("state", &"[REDACTED]")
            .field(
                "code_challenge",
                &self.code_challenge.as_ref().map(|_| "[REDACTED]"),
            )
            .field("network", &self.network)
            .finish()
    }
}

#[derive(Clone, PartialEq)]
/// Data type: identity oauth exchange context.
pub struct IdentityOAuthExchangeContext {
    /// Field: code.
    pub code: String,
    /// Field: state.
    pub state: String,
    /// Field: pkce verifier.
    pub pkce_verifier: Option<String>,
    /// Field: network.
    pub network: OAuthNetworkContext,
}

impl std::fmt::Debug for IdentityOAuthExchangeContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IdentityOAuthExchangeContext")
            .field("code", &"[REDACTED]")
            .field("state", &"[REDACTED]")
            .field(
                "pkce_verifier",
                &self.pkce_verifier.as_ref().map(|_| "[REDACTED]"),
            )
            .field("network", &self.network)
            .finish()
    }
}

#[derive(Clone, PartialEq)]
/// Data type: external identity.
pub struct ExternalIdentity {
    /// Field: provider type.
    pub provider_type: String,
    /// Field: subject.
    pub subject: String,
    /// Field: email.
    pub email: Option<String>,
    /// Field: email verified.
    pub email_verified: bool,
    /// Field: username.
    pub username: Option<String>,
    /// Field: display name.
    pub display_name: Option<String>,
    /// Field: avatar url.
    pub avatar_url: Option<String>,
    /// Field: raw.
    pub raw: Value,
}

impl std::fmt::Debug for ExternalIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExternalIdentity")
            .field("provider_type", &self.provider_type)
            .field("subject", &self.subject)
            .field("email", &self.email)
            .field("email_verified", &self.email_verified)
            .field("username", &self.username)
            .field("display_name", &self.display_name)
            .field("avatar_url", &self.avatar_url)
            .field("raw", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, PartialEq)]
/// Data type: identity claims.
pub struct IdentityClaims {
    /// Field: provider type.
    pub provider_type: String,
    /// Field: subject.
    pub subject: String,
    /// Field: email.
    pub email: Option<String>,
    /// Field: email verified.
    pub email_verified: bool,
    /// Field: username.
    pub username: Option<String>,
    /// Field: display name.
    pub display_name: Option<String>,
    /// Field: raw.
    pub raw: Value,
}

impl std::fmt::Debug for IdentityClaims {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IdentityClaims")
            .field("provider_type", &self.provider_type)
            .field("subject", &self.subject)
            .field("email", &self.email)
            .field("email_verified", &self.email_verified)
            .field("username", &self.username)
            .field("display_name", &self.display_name)
            .field("raw", &"[REDACTED]")
            .finish()
    }
}

#[async_trait]
/// Trait: identity oauth provider.
pub trait IdentityOAuthProvider: Send + Sync {
    /// Method: fn provider type.
    fn provider_type(&self) -> &'static str;

    /// Method: fn build authorize url.
    fn build_authorize_url(
        &self,
        config: &IdentityOAuthProviderConfig,
        ctx: &IdentityOAuthStartContext,
    ) -> Result<OAuthAuthorizeResponse, OAuthError>;

    /// Method: async fn exchange code.
    async fn exchange_code(
        &self,
        executor: &dyn OAuthHttpExecutor,
        config: &IdentityOAuthProviderConfig,
        ctx: &IdentityOAuthExchangeContext,
    ) -> Result<OAuthTokenSet, OAuthError>;

    /// Method: async fn fetch identity.
    async fn fetch_identity(
        &self,
        executor: &dyn OAuthHttpExecutor,
        config: &IdentityOAuthProviderConfig,
        tokens: &OAuthTokenSet,
        network: OAuthNetworkContext,
    ) -> Result<ExternalIdentity, OAuthError>;

    /// Method: fn map identity.
    fn map_identity(
        &self,
        config: &IdentityOAuthProviderConfig,
        identity: ExternalIdentity,
    ) -> Result<IdentityClaims, OAuthError>;
}

pub(crate) fn mapped_string(
    raw: &Value,
    mapping: Option<&Value>,
    logical_key: &str,
) -> Option<String> {
    let mapped_key = mapping
        .and_then(Value::as_object)
        .and_then(|object| object.get(logical_key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(logical_key);
    find_string(raw, mapped_key)
}

pub(crate) fn mapped_bool(raw: &Value, mapping: Option<&Value>, logical_key: &str) -> Option<bool> {
    let mapped_key = match mapping
        .and_then(Value::as_object)
        .and_then(|object| object.get(logical_key))
    {
        Some(value) => value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())?,
        None => logical_key,
    };
    find_value(raw, mapped_key).and_then(Value::as_bool)
}

pub(crate) fn find_string(raw: &Value, key: &str) -> Option<String> {
    find_value(raw, key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn find_value<'a>(raw: &'a Value, key: &str) -> Option<&'a Value> {
    let mut current = raw;
    for segment in key.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

pub(crate) fn form_headers() -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "content-type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        ),
        ("accept".to_string(), "application/json".to_string()),
    ])
}

/// Requires an HTTPS endpoint URL, except for HTTP on localhost or loopback
/// IPs used by local development identity providers. This mirrors the
/// gateway-side OAuth endpoint policy so the crate also fails closed when a
/// caller wires in a configuration that never went through the repository
/// validation layer: token and userinfo requests carry the client secret,
/// authorization code, and access token, so a plaintext HTTP endpoint would
/// expose all of them.
pub(crate) fn validate_identity_endpoint_url(field: &str, value: &str) -> Result<(), OAuthError> {
    let parsed = url::Url::parse(value)
        .map_err(|_| OAuthError::invalid_request(format!("{field} must be an absolute URL")))?;
    let is_loopback = match parsed.host() {
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    };
    if parsed.scheme() != "https" && !(parsed.scheme() == "http" && is_loopback) {
        return Err(OAuthError::invalid_request(format!(
            "{field} must use https, except for localhost or loopback IPs"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        mapped_bool, validate_identity_endpoint_url, ExternalIdentity, IdentityClaims,
        IdentityOAuthExchangeContext, IdentityOAuthProviderConfig, IdentityOAuthStartContext,
    };
    use crate::network::OAuthNetworkContext;
    use serde_json::json;

    #[test]
    fn identity_oauth_debug_output_redacts_credentials_and_raw_claims() {
        let config = IdentityOAuthProviderConfig {
            provider_type: "custom".to_string(),
            display_name: "Custom".to_string(),
            authorization_url: "https://idp.example/authorize".to_string(),
            token_url: "https://idp.example/token".to_string(),
            userinfo_url: Some("https://idp.example/userinfo".to_string()),
            client_id: "public-client".to_string(),
            client_secret: Some("identity-client-secret-canary".to_string()),
            scopes: vec!["openid".to_string()],
            redirect_uri: "https://gateway.example/callback".to_string(),
            frontend_callback_url: "https://app.example/callback".to_string(),
            attribute_mapping: None,
            extra_config: Some(json!({"secret": "identity-extra-canary"})),
        };
        let start = IdentityOAuthStartContext {
            state: "identity-state-canary".to_string(),
            code_challenge: Some("identity-challenge-canary".to_string()),
            network: OAuthNetworkContext::direct_identity(),
        };
        let exchange = IdentityOAuthExchangeContext {
            code: "identity-code-canary".to_string(),
            state: "identity-exchange-state-canary".to_string(),
            pkce_verifier: Some("identity-verifier-canary".to_string()),
            network: OAuthNetworkContext::direct_identity(),
        };
        let external = ExternalIdentity {
            provider_type: "custom".to_string(),
            subject: "subject".to_string(),
            email: None,
            email_verified: false,
            username: None,
            display_name: None,
            avatar_url: None,
            raw: json!({"access_token": "identity-raw-canary"}),
        };
        let claims = IdentityClaims {
            provider_type: "custom".to_string(),
            subject: "subject".to_string(),
            email: None,
            email_verified: false,
            username: None,
            display_name: None,
            raw: json!({"id_token": "identity-claims-canary"}),
        };

        let debug = format!("{config:?} {start:?} {exchange:?} {external:?} {claims:?}");
        for secret in [
            "identity-client-secret-canary",
            "identity-extra-canary",
            "identity-state-canary",
            "identity-challenge-canary",
            "identity-code-canary",
            "identity-exchange-state-canary",
            "identity-verifier-canary",
            "identity-raw-canary",
            "identity-claims-canary",
        ] {
            assert!(!debug.contains(secret), "debug leaked {secret}");
        }
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn identity_endpoint_url_requires_https_except_loopback() {
        for value in [
            "https://idp.example.test/token",
            "https://accounts.idp.example.test/userinfo?schema=current",
            "http://localhost:8080/token",
            "http://localhost/token",
            "http://127.0.0.1:9090/userinfo",
            "http://[::1]:8080/token",
        ] {
            assert!(
                validate_identity_endpoint_url("token_url", value).is_ok(),
                "rejected {value}"
            );
        }

        for value in [
            "http://idp.example.test/token",
            "http://192.168.1.10/token",
            "http://[2001:db8::1]/token",
            "ftp://idp.example.test/token",
            "idp.example.test/token",
            "",
        ] {
            assert!(
                validate_identity_endpoint_url("token_url", value).is_err(),
                "accepted {value}"
            );
        }
    }

    #[test]
    fn mapped_bool_accepts_only_an_explicit_json_boolean() {
        let raw = json!({
            "email_verified": true,
            "profile": {
                "verified": false,
                "string_verified": "true",
                "numeric_verified": 1
            }
        });

        assert_eq!(mapped_bool(&raw, None, "email_verified"), Some(true));
        assert_eq!(
            mapped_bool(
                &raw,
                Some(&json!({"email_verified": "profile.verified"})),
                "email_verified"
            ),
            Some(false)
        );
        assert_eq!(
            mapped_bool(
                &raw,
                Some(&json!({"email_verified": "profile.string_verified"})),
                "email_verified"
            ),
            None
        );
        assert_eq!(
            mapped_bool(
                &raw,
                Some(&json!({"email_verified": "profile.numeric_verified"})),
                "email_verified"
            ),
            None
        );
        assert_eq!(mapped_bool(&json!({}), None, "email_verified"), None);
        assert_eq!(
            mapped_bool(
                &raw,
                Some(&json!({"email_verified": true})),
                "email_verified"
            ),
            None
        );
        assert_eq!(
            mapped_bool(&raw, Some(&json!({"email_verified": ""})), "email_verified"),
            None
        );
    }
}
