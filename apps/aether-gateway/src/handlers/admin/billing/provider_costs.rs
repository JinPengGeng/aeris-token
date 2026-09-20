use super::{
    admin_billing_parse_page, admin_billing_parse_page_size,
    build_admin_billing_bad_request_response, build_admin_billing_data_unavailable_response,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{attach_admin_audit_response, query_param_value};
use crate::GatewayError;
use aether_data::repository::provider_cost::{
    ProviderCostListQuery, ProviderCostPrice, ProviderCostSnapshotImport, ProviderCostSummaryQuery,
};
use aether_data::DataLayerError;
use axum::{
    body::{Body, Bytes},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;

const MAX_PROVIDER_COST_IMPORT_ITEMS: usize = 100;

#[derive(Debug, Deserialize)]
struct ProviderCostPriceImportRequest {
    prices: Vec<ProviderCostPrice>,
}

#[derive(Debug, Deserialize)]
struct ProviderCostSnapshotImportRequest {
    snapshots: Vec<ProviderCostSnapshotImport>,
}

fn import_operator_id(request_context: &AdminRequestContext<'_>) -> String {
    request_context
        .decision()
        .and_then(|decision| decision.admin_principal.as_ref())
        .map(|principal| principal.user_id.clone())
        .unwrap_or_else(|| "management_token".to_string())
}

fn parse_provider_cost_import_body<T: serde::de::DeserializeOwned>(
    request_body: Option<&Bytes>,
    collection_key: &str,
) -> Result<T, Response<Body>> {
    let body = request_body
        .filter(|body| !body.is_empty())
        .ok_or_else(|| {
            build_admin_billing_bad_request_response("request body must be a JSON object")
        })?;
    let mut value: serde_json::Value = serde_json::from_slice(body)
        .map_err(|_| build_admin_billing_bad_request_response("request body is invalid"))?;
    let items = value
        .get_mut(collection_key)
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| {
            build_admin_billing_bad_request_response(format!("{collection_key} must be an array"))
        })?;
    for item in items {
        let object = item.as_object_mut().ok_or_else(|| {
            build_admin_billing_bad_request_response(format!(
                "{collection_key} must contain objects"
            ))
        })?;
        object
            .entry("imported_by")
            .or_insert_with(|| serde_json::Value::String(String::new()));
    }
    serde_json::from_value(value)
        .map_err(|_| build_admin_billing_bad_request_response("request body is invalid"))
}

fn provider_cost_import_failure_response<T: serde::Serialize>(
    outcomes: &[T],
    failed_index: usize,
    error: DataLayerError,
) -> Response<Body> {
    let (status, detail) = match error {
        DataLayerError::InvalidInput(detail)
            if detail.contains("already has different content") =>
        {
            (
                axum::http::StatusCode::CONFLICT,
                "provider cost import conflicts with an existing import id",
            )
        }
        DataLayerError::InvalidInput(_) => (
            axum::http::StatusCode::BAD_REQUEST,
            "provider cost import was rejected",
        ),
        _ => (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "provider cost import is temporarily unavailable",
        ),
    };
    (
        status,
        Json(json!({
            "detail": detail,
            "partial": !outcomes.is_empty(),
            "completed": outcomes,
            "failed_index": failed_index,
            "retry_safe": true,
        })),
    )
        .into_response()
}

fn validate_import_batch<T>(items: &[T]) -> Result<(), Response<Body>> {
    if items.is_empty() {
        return Err(build_admin_billing_bad_request_response(
            "import batch must contain at least one item",
        ));
    }
    if items.len() > MAX_PROVIDER_COST_IMPORT_ITEMS {
        return Err(build_admin_billing_bad_request_response(format!(
            "import batch must contain at most {MAX_PROVIDER_COST_IMPORT_ITEMS} items"
        )));
    }
    Ok(())
}

fn parse_required_unix_secs(query: Option<&str>, key: &str) -> Result<u64, Response<Body>> {
    query_param_value(query, key)
        .ok_or_else(|| build_admin_billing_bad_request_response(format!("{key} is required")))?
        .parse::<u64>()
        .map_err(|_| {
            build_admin_billing_bad_request_response(format!("{key} must be a unix timestamp"))
        })
}

pub(super) async fn maybe_build_local_admin_provider_costs_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    match request_context.route_kind() {
        Some("list_provider_cost_prices") => Ok(Some(
            build_list_provider_cost_prices_response(state, request_context).await?,
        )),
        Some("import_provider_cost_prices") => Ok(Some(
            build_import_provider_cost_prices_response(state, request_context, request_body)
                .await?,
        )),
        Some("summarize_provider_cost_snapshots") => Ok(Some(
            build_provider_cost_snapshot_summary_response(state, request_context).await?,
        )),
        Some("import_provider_cost_snapshots") => Ok(Some(
            build_import_provider_cost_snapshots_response(state, request_context, request_body)
                .await?,
        )),
        _ => Ok(None),
    }
}

async fn build_list_provider_cost_prices_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let page = match admin_billing_parse_page(request_context.query_string()) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_billing_bad_request_response(detail)),
    };
    let page_size = match admin_billing_parse_page_size(request_context.query_string()) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_billing_bad_request_response(detail)),
    };
    let query = ProviderCostListQuery {
        limit: page_size,
        offset: page.saturating_sub(1).saturating_mul(page_size),
    };
    match state.app().list_provider_cost_prices(&query).await? {
        Some(items) => Ok(
            Json(json!({ "items": items, "page": page, "page_size": page_size })).into_response(),
        ),
        None => Ok(build_admin_billing_data_unavailable_response()),
    }
}

async fn build_import_provider_cost_prices_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let mut input: ProviderCostPriceImportRequest =
        match parse_provider_cost_import_body(request_body, "prices") {
            Ok(value) => value,
            Err(response) => return Ok(response),
        };
    if let Err(response) = validate_import_batch(&input.prices) {
        return Ok(response);
    }
    let imported_by = import_operator_id(request_context);
    for price in &mut input.prices {
        price.imported_by.clone_from(&imported_by);
        if let Err(error) = price.validate() {
            return Ok(build_admin_billing_bad_request_response(error.to_string()));
        }
    }

    let mut outcomes = Vec::with_capacity(input.prices.len());
    for price in &input.prices {
        let outcome = match state.app().import_provider_cost_price(price).await {
            Ok(Some(outcome)) => outcome,
            Ok(None) => return Ok(build_admin_billing_data_unavailable_response()),
            Err(error) => {
                let response =
                    provider_cost_import_failure_response(&outcomes, outcomes.len(), error);
                return Ok(if outcomes.is_empty() {
                    response
                } else {
                    attach_admin_audit_response(
                        response,
                        "admin_provider_cost_prices_import_partially_completed",
                        "import_provider_cost_prices",
                        "provider_cost_price_import",
                        &format!("batch:completed={}", outcomes.len()),
                    )
                });
            }
        };
        outcomes.push(outcome);
    }
    let inserted = outcomes.iter().filter(|outcome| outcome.inserted).count();
    let response = Json(json!({
        "items": outcomes,
        "inserted": inserted,
        "already_exists": input.prices.len() - inserted,
    }))
    .into_response();
    Ok(attach_admin_audit_response(
        response,
        "admin_provider_cost_prices_imported",
        "import_provider_cost_prices",
        "provider_cost_price_import",
        &format!("batch:count={}", input.prices.len()),
    ))
}

async fn build_provider_cost_snapshot_summary_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let occurred_from_unix_secs =
        match parse_required_unix_secs(request_context.query_string(), "from") {
            Ok(value) => value,
            Err(response) => return Ok(response),
        };
    let occurred_until_unix_secs =
        match parse_required_unix_secs(request_context.query_string(), "until") {
            Ok(value) => value,
            Err(response) => return Ok(response),
        };
    if occurred_until_unix_secs <= occurred_from_unix_secs {
        return Ok(build_admin_billing_bad_request_response(
            "until must be later than from",
        ));
    }
    let query = ProviderCostSummaryQuery {
        occurred_from_unix_secs,
        occurred_until_unix_secs,
    };
    match state
        .app()
        .summarize_provider_cost_snapshots(&query)
        .await?
    {
        Some(items) => Ok(Json(json!({
            "items": items,
            "occurred_from_unix_secs": occurred_from_unix_secs,
            "occurred_until_unix_secs": occurred_until_unix_secs,
        }))
        .into_response()),
        None => Ok(build_admin_billing_data_unavailable_response()),
    }
}

async fn build_import_provider_cost_snapshots_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let mut input: ProviderCostSnapshotImportRequest =
        match parse_provider_cost_import_body(request_body, "snapshots") {
            Ok(value) => value,
            Err(response) => return Ok(response),
        };
    if let Err(response) = validate_import_batch(&input.snapshots) {
        return Ok(response);
    }
    let imported_by = import_operator_id(request_context);
    for snapshot in &mut input.snapshots {
        snapshot.imported_by.clone_from(&imported_by);
        if let Err(error) = snapshot.validate() {
            return Ok(build_admin_billing_bad_request_response(error.to_string()));
        }
    }

    let mut outcomes = Vec::with_capacity(input.snapshots.len());
    for snapshot in &input.snapshots {
        let outcome = match state.app().import_provider_cost_snapshot(snapshot).await {
            Ok(Some(outcome)) => outcome,
            Ok(None) => return Ok(build_admin_billing_data_unavailable_response()),
            Err(error) => {
                let response =
                    provider_cost_import_failure_response(&outcomes, outcomes.len(), error);
                return Ok(if outcomes.is_empty() {
                    response
                } else {
                    attach_admin_audit_response(
                        response,
                        "admin_provider_cost_snapshots_import_partially_completed",
                        "import_provider_cost_snapshots",
                        "provider_cost_snapshot_import",
                        &format!("batch:completed={}", outcomes.len()),
                    )
                });
            }
        };
        outcomes.push(outcome);
    }
    let inserted = outcomes.iter().filter(|outcome| outcome.inserted).count();
    let response = Json(json!({
        "items": outcomes,
        "inserted": inserted,
        "already_exists": input.snapshots.len() - inserted,
    }))
    .into_response();
    Ok(attach_admin_audit_response(
        response,
        "admin_provider_cost_snapshots_imported",
        "import_provider_cost_snapshots",
        "provider_cost_snapshot_import",
        &format!("batch:count={}", input.snapshots.len()),
    ))
}
