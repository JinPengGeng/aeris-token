//! OAuth 2.0 / PKCE integration core for Aether: provider adapters, token
//! flows, and the HTTP policy used to reach external identity providers.
#![warn(missing_docs)]

/// Module: core.
pub mod core;
/// Module: identity.
pub mod identity;
/// Module: network.
pub mod network;
/// Module: provider.
pub mod provider;

pub use core::{
    current_unix_secs, generate_oauth_nonce, generate_pkce_verifier, parse_oauth_callback_params,
    pkce_s256, redacted_oauth_error_body_excerpt, OAuthAdapterRegistry, OAuthAuthorizeRequest,
    OAuthAuthorizeResponse, OAuthCallback, OAuthError, OAuthProviderMetadata, OAuthTokenSet,
};
pub use network::{
    NetworkRequirement, OAuthHttpExecutor, OAuthHttpRequest, OAuthHttpResponse,
    OAuthNetworkContext, OAuthNetworkPolicy, OAuthTimeouts,
};
