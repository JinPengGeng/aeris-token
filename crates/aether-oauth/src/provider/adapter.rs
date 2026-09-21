use super::{
    ProviderOAuthAccount, ProviderOAuthAccountState, ProviderOAuthCapabilities,
    ProviderOAuthCookieAuthorizationInput, ProviderOAuthImportInput, ProviderOAuthRequestAuth,
    ProviderOAuthTokenSet, ProviderOAuthTransportContext,
};
use crate::core::{OAuthAuthorizeResponse, OAuthError};
use crate::network::OAuthHttpExecutor;
use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq)]
/// Data type: provider oauth probe result.
pub struct ProviderOAuthProbeResult {
/// Field: state.
    pub state: ProviderOAuthAccountState,
}

/// Trait: provider oauth adapter.
#[async_trait]
pub trait ProviderOAuthAdapter: Send + Sync {
    /// Returns the stable provider type key this adapter handles.
    fn provider_type(&self) -> &'static str;

    /// Returns the OAuth capabilities supported by this provider.
    fn capabilities(&self) -> ProviderOAuthCapabilities;

    /// Builds the provider authorization URL; the default implementation
    /// reports an unsupported provider.
    fn build_authorize_url(
        &self,
        _ctx: &ProviderOAuthTransportContext,
        _state: &str,
        _code_challenge: Option<&str>,
    ) -> Result<OAuthAuthorizeResponse, OAuthError> {
        Err(OAuthError::UnsupportedProvider(
            self.provider_type().to_string(),
        ))
    }

    /// Exchanges an authorization code for a token set; the default
    /// implementation reports an unsupported provider.
    async fn exchange_code(
        &self,
        _executor: &dyn OAuthHttpExecutor,
        _ctx: &ProviderOAuthTransportContext,
        _code: &str,
        _state: &str,
        _pkce_verifier: Option<&str>,
    ) -> Result<ProviderOAuthTokenSet, OAuthError> {
        Err(OAuthError::UnsupportedProvider(
            self.provider_type().to_string(),
        ))
    }

    /// Authorizes via a stored provider cookie instead of a redirect code
    /// exchange; the default implementation reports an unsupported provider.
    async fn authorize_with_cookie(
        &self,
        _executor: &dyn OAuthHttpExecutor,
        _ctx: &ProviderOAuthTransportContext,
        _input: ProviderOAuthCookieAuthorizationInput,
    ) -> Result<ProviderOAuthTokenSet, OAuthError> {
        Err(OAuthError::UnsupportedProvider(
            self.provider_type().to_string(),
        ))
    }

    /// Imports externally obtained credentials into a normalized token set.
    async fn import_credentials(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        input: ProviderOAuthImportInput,
    ) -> Result<ProviderOAuthTokenSet, OAuthError>;

    /// Refreshes an expired account token set using the provider's refresh flow.
    async fn refresh(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        account: &ProviderOAuthAccount,
    ) -> Result<ProviderOAuthTokenSet, OAuthError>;

    /// Resolves the request authorization material for a stored account.
    fn resolve_request_auth(
        &self,
        account: &ProviderOAuthAccount,
    ) -> Result<ProviderOAuthRequestAuth, OAuthError>;

    /// Returns a stable fingerprint identifying the account, if supported.
    fn account_fingerprint(&self, account: &ProviderOAuthAccount) -> Option<String>;

    /// Probes the account state at the provider; the default returns `Ok(None)`.
    async fn probe_account_state(
        &self,
        _executor: &dyn OAuthHttpExecutor,
        _ctx: &ProviderOAuthTransportContext,
        _account: &ProviderOAuthAccount,
    ) -> Result<Option<ProviderOAuthProbeResult>, OAuthError> {
        Ok(None)
    }
}
