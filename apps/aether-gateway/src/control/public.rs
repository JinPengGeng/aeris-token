use axum::http::Uri;

use crate::{AppState, GatewayError};

use super::{
    resolve_control_route, route::resolve_control_route_with_trusted_auth, GatewayControlDecision,
};

pub(crate) type GatewayPublicRequestContext =
    aether_gateway_control::PublicRequestContext<GatewayControlDecision>;

fn build_public_request_context(
    trace_id: &str,
    method: &http::Method,
    uri: &Uri,
    headers: &http::HeaderMap,
    control_decision: Option<GatewayControlDecision>,
) -> GatewayPublicRequestContext {
    let mut context = GatewayPublicRequestContext::from_request_parts(
        trace_id,
        method,
        uri,
        headers,
        control_decision,
    );
    if let Some(decision) = context.control_decision.as_ref() {
        context.request_path = decision.public_path.clone();
    }
    context
}

pub(crate) async fn resolve_public_request_context(
    state: &AppState,
    method: &http::Method,
    uri: &Uri,
    headers: &http::HeaderMap,
    trace_id: &str,
) -> Result<GatewayPublicRequestContext, GatewayError> {
    let control_decision = resolve_control_route(state, method, uri, headers, trace_id).await?;
    Ok(build_public_request_context(
        trace_id,
        method,
        uri,
        headers,
        control_decision,
    ))
}

pub(crate) async fn resolve_public_request_context_with_trusted_auth(
    state: &AppState,
    method: &http::Method,
    uri: &Uri,
    headers: &http::HeaderMap,
    trace_id: &str,
) -> Result<GatewayPublicRequestContext, GatewayError> {
    let control_decision =
        resolve_control_route_with_trusted_auth(state, method, uri, headers, trace_id, true)
            .await?;
    Ok(build_public_request_context(
        trace_id,
        method,
        uri,
        headers,
        control_decision,
    ))
}

pub(crate) async fn resolve_public_request_context_without_trusted_auth(
    state: &AppState,
    method: &http::Method,
    uri: &Uri,
    headers: &http::HeaderMap,
    trace_id: &str,
) -> Result<GatewayPublicRequestContext, GatewayError> {
    let control_decision =
        resolve_control_route_with_trusted_auth(state, method, uri, headers, trace_id, false)
            .await?;
    Ok(build_public_request_context(
        trace_id,
        method,
        uri,
        headers,
        control_decision,
    ))
}
