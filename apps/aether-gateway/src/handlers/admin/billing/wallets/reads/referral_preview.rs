use super::super::shared::{
    admin_wallet_refund_ids_from_suffix_path, build_admin_wallet_not_found_response,
    build_admin_wallet_refund_not_found_response, build_admin_wallets_bad_request_response,
    build_admin_wallets_data_unavailable_response, ADMIN_WALLETS_API_KEY_REFUND_DETAIL,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// Read-only referral clawback preview for a refund request: which inviter
/// rewards would be reversed, how much of each is covered by the inviter's
/// gift balance, and how much would remain as pending reversal debt. Never
/// mutates anything; the real clawback still runs in the refund flow via
/// `reverse_referral_rewards_for_order`.
pub(in super::super) async fn build_admin_wallet_refund_referral_preview_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some((wallet_id, refund_id)) =
        admin_wallet_refund_ids_from_suffix_path(request_context.path(), "/referral-preview")
    else {
        return Ok(build_admin_wallets_bad_request_response(
            "wallet_id 或 refund_id 无效",
        ));
    };

    let Some(existing_wallet) = state
        .find_wallet(aether_data::repository::wallet::WalletLookupKey::WalletId(
            &wallet_id,
        ))
        .await?
    else {
        return Ok(build_admin_wallet_not_found_response());
    };
    if existing_wallet.api_key_id.is_some() {
        return Ok(build_admin_wallets_bad_request_response(
            ADMIN_WALLETS_API_KEY_REFUND_DETAIL,
        ));
    }

    let Some(refund) = state.find_wallet_refund(&wallet_id, &refund_id).await? else {
        return Ok(build_admin_wallet_refund_not_found_response());
    };

    let Some(order_id) = refund.payment_order_id.clone() else {
        return Ok(Json(json!({
            "refund_id": refund.id,
            "payment_order_id": serde_json::Value::Null,
            "applicable": false,
            "note": "该退款未关联支付订单，无返利冲回影响",
            "total_expected_reversal_usd": 0.0,
            "total_deductible_usd": 0.0,
            "total_shortfall_usd": 0.0,
            "rewards": [],
        }))
        .into_response());
    };

    // A succeeded refund has no outstanding amount left to claw back; its
    // preview reflects the already-refunded state only.
    let refund_amount_usd = if refund.status == "succeeded" {
        0.0
    } else {
        refund.amount_usd
    };

    if !state.app().has_referral_data_backend() {
        return Ok(build_admin_wallets_data_unavailable_response());
    }

    let items = state
        .app()
        .preview_referral_clawback(&order_id, refund_amount_usd)
        .await?;
    let sum = |key: &str| {
        items
            .iter()
            .filter_map(|item| serde_json::to_value(item).ok())
            .filter_map(|value| value[key].as_f64())
            .sum::<f64>()
    };
    Ok(Json(json!({
        "refund_id": refund.id,
        "payment_order_id": order_id,
        "refund_amount_usd": refund_amount_usd,
        "applicable": true,
        "total_expected_reversal_usd": sum("expected_reversal_amount_usd"),
        "total_deductible_usd": sum("deductible_amount_usd"),
        "total_shortfall_usd": sum("shortfall_amount_usd"),
        "rewards": items,
    }))
    .into_response())
}
