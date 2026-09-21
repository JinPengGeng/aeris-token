use super::{maybe_build_local_admin_billing_response, payments, wallets, writeoffs};
use crate::handlers::admin::request::{AdminRouteRequest, AdminRouteResult};

pub(crate) async fn maybe_build_local_admin_billing_routes_response(
    request: AdminRouteRequest<'_>,
) -> AdminRouteResult {
    if let Some(response) = maybe_build_local_admin_billing_response(
        &request.state(),
        &request.request_context(),
        request.request_body(),
    )
    .await?
    {
        return Ok(Some(response));
    }

    if let Some(response) = payments::maybe_build_local_admin_payments_response(
        &request.state(),
        &request.request_context(),
        request.request_body(),
    )
    .await?
    {
        return Ok(Some(response));
    }

    if let Some(response) =
        writeoffs::maybe_build_local_admin_insufficient_quota_writeoffs_response(
            &request.state(),
            &request.request_context(),
        )
        .await?
    {
        return Ok(Some(response));
    }

    if let Some(response) = wallets::maybe_build_local_admin_wallets_response(
        &request.state(),
        &request.request_context(),
        request.request_body(),
    )
    .await?
    {
        return Ok(Some(response));
    }

    Ok(None)
}
