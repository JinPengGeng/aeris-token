use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq)]
/// Data type: oauth provider metadata.
pub struct OAuthProviderMetadata {
    /// Field: provider type.
    pub provider_type: String,
    /// Field: display name.
    pub display_name: String,
    /// Field: authorize url.
    pub authorize_url: String,
    /// Field: token url.
    pub token_url: String,
    /// Field: client id.
    pub client_id: String,
    /// Field: client secret.
    pub client_secret: Option<String>,
    /// Field: scopes.
    pub scopes: Vec<String>,
    /// Field: redirect uri.
    pub redirect_uri: String,
    /// Field: use pkce.
    pub use_pkce: bool,
}

impl std::fmt::Debug for OAuthProviderMetadata {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OAuthProviderMetadata")
            .field("provider_type", &self.provider_type)
            .field("display_name", &self.display_name)
            .field("authorize_url", &"[REDACTED]")
            .field("token_url", &"[REDACTED]")
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "[REDACTED]"),
            )
            .field("scopes", &self.scopes)
            .field("redirect_uri", &self.redirect_uri)
            .field("use_pkce", &self.use_pkce)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
/// Data type: oauth authorize request.
pub struct OAuthAuthorizeRequest {
    /// Field: state.
    pub state: String,
    /// Field: code challenge.
    pub code_challenge: Option<String>,
    /// Field: prompt.
    pub prompt: Option<String>,
    /// Field: login hint.
    pub login_hint: Option<String>,
}

impl std::fmt::Debug for OAuthAuthorizeRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OAuthAuthorizeRequest")
            .field("state", &"[REDACTED]")
            .field(
                "code_challenge",
                &self.code_challenge.as_ref().map(|_| "[REDACTED]"),
            )
            .field("prompt", &self.prompt)
            .field("has_login_hint", &self.login_hint.is_some())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize)]
/// Data type: oauth authorize response.
pub struct OAuthAuthorizeResponse {
    /// Field: authorize url.
    pub authorize_url: String,
    /// Field: state.
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Field: code challenge.
    pub code_challenge: Option<String>,
}

impl std::fmt::Debug for OAuthAuthorizeResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OAuthAuthorizeResponse")
            .field("authorize_url", &"[REDACTED]")
            .field("state", &"[REDACTED]")
            .field(
                "code_challenge",
                &self.code_challenge.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
/// Data type: oauth callback.
pub struct OAuthCallback {
    /// Field: code.
    pub code: String,
    /// Field: state.
    pub state: String,
    /// Field: scope.
    pub scope: Option<String>,
}

impl std::fmt::Debug for OAuthCallback {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OAuthCallback")
            .field("code", &"[REDACTED]")
            .field("state", &"[REDACTED]")
            .field("scope", &self.scope)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
/// Data type: oauth device authorization.
pub struct OAuthDeviceAuthorization {
    /// Field: device code.
    pub device_code: String,
    /// Field: user code.
    pub user_code: String,
    /// Field: verification uri.
    pub verification_uri: String,
    /// Field: verification uri complete.
    pub verification_uri_complete: String,
    /// Field: expires in.
    pub expires_in: u64,
    /// Field: interval.
    pub interval: u64,
}

impl std::fmt::Debug for OAuthDeviceAuthorization {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OAuthDeviceAuthorization")
            .field("device_code", &"[REDACTED]")
            .field("user_code", &"[REDACTED]")
            .field("verification_uri", &self.verification_uri)
            .field("verification_uri_complete", &"[REDACTED]")
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OAuthAuthorizeRequest, OAuthAuthorizeResponse, OAuthCallback, OAuthDeviceAuthorization,
        OAuthProviderMetadata,
    };

    #[test]
    fn oauth_flow_debug_output_redacts_capabilities_and_secrets() {
        let metadata = OAuthProviderMetadata {
            provider_type: "test".to_string(),
            display_name: "Test".to_string(),
            authorize_url: "https://idp.example/authorize".to_string(),
            token_url: "https://idp.example/token".to_string(),
            client_id: "public-client".to_string(),
            client_secret: Some("client-secret-canary".to_string()),
            scopes: vec!["openid".to_string()],
            redirect_uri: "https://gateway.example/callback".to_string(),
            use_pkce: true,
        };
        let request = OAuthAuthorizeRequest {
            state: "state-canary".to_string(),
            code_challenge: Some("challenge-canary".to_string()),
            prompt: None,
            login_hint: Some("login-hint-canary".to_string()),
        };
        let response = OAuthAuthorizeResponse {
            authorize_url:
                "https://idp.example/authorize?state=state-url-canary&code_challenge=challenge"
                    .to_string(),
            state: "response-state-canary".to_string(),
            code_challenge: Some("response-challenge-canary".to_string()),
        };
        let callback = OAuthCallback {
            code: "authorization-code-canary".to_string(),
            state: "callback-state-canary".to_string(),
            scope: None,
        };
        let device = OAuthDeviceAuthorization {
            device_code: "device-code-canary".to_string(),
            user_code: "user-code-canary".to_string(),
            verification_uri: "https://idp.example/device".to_string(),
            verification_uri_complete: "https://idp.example/device?code=complete-canary"
                .to_string(),
            expires_in: 600,
            interval: 5,
        };

        let debug = format!("{metadata:?} {request:?} {response:?} {callback:?} {device:?}");
        for secret in [
            "client-secret-canary",
            "state-canary",
            "challenge-canary",
            "login-hint-canary",
            "state-url-canary",
            "response-state-canary",
            "response-challenge-canary",
            "authorization-code-canary",
            "callback-state-canary",
            "device-code-canary",
            "user-code-canary",
            "complete-canary",
        ] {
            assert!(!debug.contains(secret), "debug leaked {secret}");
        }
        assert!(debug.contains("[REDACTED]"));
    }
}
