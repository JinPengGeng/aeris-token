use super::{
    IdentityClaims, IdentityOAuthExchangeContext, IdentityOAuthProvider,
    IdentityOAuthProviderConfig, IdentityOAuthStartContext,
};
use crate::core::{OAuthAdapterRegistry, OAuthAuthorizeResponse, OAuthError};
use crate::network::OAuthHttpExecutor;
use std::sync::Arc;

#[derive(Debug, Clone, Default)]
/// Data type: identity oauth service.
pub struct IdentityOAuthService {
    registry: OAuthAdapterRegistry<dyn IdentityOAuthProvider>,
}

#[derive(Debug, Clone, PartialEq)]
/// Data type: oauth login outcome.
pub struct OAuthLoginOutcome {
/// Field: claims.
    pub claims: IdentityClaims,
/// Field: is new external identity.
    pub is_new_external_identity: bool,
}

#[derive(Debug, Clone, PartialEq)]
/// Data type: bound oauth identity.
pub struct BoundOAuthIdentity {
/// Field: claims.
    pub claims: IdentityClaims,
/// Field: replaced existing binding.
    pub replaced_existing_binding: bool,
}

impl IdentityOAuthService {
/// Constructor / associated function: new.
    pub fn new() -> Self {
        Self::default()
    }

/// Constructor / associated function: with builtin providers.
    pub fn with_builtin_providers() -> Self {
        use super::providers::{CustomOidcIdentityOAuthProvider, LinuxDoIdentityOAuthProvider};

        Self::new()
            .with_provider(Arc::new(LinuxDoIdentityOAuthProvider::default()))
            .with_provider(Arc::new(CustomOidcIdentityOAuthProvider))
    }

/// Method: with provider.
    pub fn with_provider(mut self, provider: Arc<dyn IdentityOAuthProvider>) -> Self {
        self.registry.insert(provider.provider_type(), provider);
        self
    }

/// Method: provider.
    pub fn provider(
        &self,
        provider_type: &str,
    ) -> Result<Arc<dyn IdentityOAuthProvider>, OAuthError> {
        self.registry
            .get(provider_type)
            .or_else(|| {
                is_custom_oidc_provider_type(provider_type)
                    .then(|| self.registry.get("custom_oidc"))
                    .flatten()
            })
            .ok_or_else(|| OAuthError::UnsupportedProvider(provider_type.to_string()))
    }

/// Method: start.
    pub fn start(
        &self,
        config: &IdentityOAuthProviderConfig,
        ctx: &IdentityOAuthStartContext,
    ) -> Result<OAuthAuthorizeResponse, OAuthError> {
        self.provider(&config.provider_type)?
            .build_authorize_url(config, ctx)
    }

/// Method: login.
    pub async fn login(
        &self,
        executor: &dyn OAuthHttpExecutor,
        config: &IdentityOAuthProviderConfig,
        ctx: &IdentityOAuthExchangeContext,
    ) -> Result<OAuthLoginOutcome, OAuthError> {
        let provider = self.provider(&config.provider_type)?;
        login_with_oauth(provider.as_ref(), executor, config, ctx).await
    }

/// Method: bind.
    pub async fn bind(
        &self,
        executor: &dyn OAuthHttpExecutor,
        config: &IdentityOAuthProviderConfig,
        ctx: &IdentityOAuthExchangeContext,
    ) -> Result<BoundOAuthIdentity, OAuthError> {
        let provider = self.provider(&config.provider_type)?;
        bind_oauth_identity(provider.as_ref(), executor, config, ctx).await
    }
}

fn is_custom_oidc_provider_type(provider_type: &str) -> bool {
    let normalized = provider_type.trim().to_ascii_lowercase();
    normalized == "custom_oidc"
        || normalized.starts_with("custom_oidc_")
        || normalized.starts_with("custom_")
        || normalized.starts_with("oidc_")
}

/// Function: start identity oauth.
pub fn start_identity_oauth(
    provider: &dyn IdentityOAuthProvider,
    config: &IdentityOAuthProviderConfig,
    ctx: &IdentityOAuthStartContext,
) -> Result<OAuthAuthorizeResponse, OAuthError> {
    provider.build_authorize_url(config, ctx)
}

/// Function: login with oauth.
pub async fn login_with_oauth(
    provider: &dyn IdentityOAuthProvider,
    executor: &dyn OAuthHttpExecutor,
    config: &IdentityOAuthProviderConfig,
    ctx: &IdentityOAuthExchangeContext,
) -> Result<OAuthLoginOutcome, OAuthError> {
    let tokens = provider.exchange_code(executor, config, ctx).await?;
    let identity = provider
        .fetch_identity(executor, config, &tokens, ctx.network.clone())
        .await?;
    let claims = provider.map_identity(config, identity)?;
    Ok(OAuthLoginOutcome {
        claims,
        is_new_external_identity: false,
    })
}

/// Function: bind oauth identity.
pub async fn bind_oauth_identity(
    provider: &dyn IdentityOAuthProvider,
    executor: &dyn OAuthHttpExecutor,
    config: &IdentityOAuthProviderConfig,
    ctx: &IdentityOAuthExchangeContext,
) -> Result<BoundOAuthIdentity, OAuthError> {
    let tokens = provider.exchange_code(executor, config, ctx).await?;
    let identity = provider
        .fetch_identity(executor, config, &tokens, ctx.network.clone())
        .await?;
    let claims = provider.map_identity(config, identity)?;
    Ok(BoundOAuthIdentity {
        claims,
        replaced_existing_binding: false,
    })
}

#[cfg(test)]
mod tests {
    use super::IdentityOAuthService;

    #[test]
    fn builtin_identity_service_registers_login_and_custom_oidc_providers() {
        let service = IdentityOAuthService::with_builtin_providers();

        assert!(service.provider("linuxdo").is_ok());
        assert!(service.provider("custom_oidc").is_ok());
        assert!(service.provider("custom_oidc_work").is_ok());
        assert!(service.provider("missing").is_err());
    }
}
