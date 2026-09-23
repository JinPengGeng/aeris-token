use super::{
    admin_billing_optional_filter, admin_billing_parse_page, admin_billing_parse_page_size,
    build_admin_billing_bad_request_response, build_admin_billing_data_unavailable_response,
    build_admin_billing_not_found_response,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::GatewayError;
use aether_data::repository::provider_cost_catalog::{
    ProviderCostCatalogDeleteOutcome, ProviderCostCatalogListQuery, ProviderCostCatalogRecord,
    ProviderCostCatalogUpsertOutcome, ProviderCostTaskType, PROVIDER_COST_CATALOG_MAX_LIST_LIMIT,
};
use axum::{
    body::{Body, Bytes},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;

const PROVIDER_COST_CATALOG_PATH: &str = "/api/admin/billing/provider-cost-catalogs";

#[derive(Debug, Deserialize)]
struct ProviderCostCatalogWriteRequest {
    provider_id: String,
    model: String,
    task_type: String,
    #[serde(default = "default_provider_cost_catalog_currency")]
    currency: String,
    #[serde(default)]
    price_per_request: Option<f64>,
    #[serde(default)]
    tiered_pricing: Option<serde_json::Value>,
    effective_from_unix_secs: u64,
    #[serde(default)]
    effective_to_unix_secs: Option<u64>,
}

fn default_provider_cost_catalog_currency() -> String {
    "USD".to_string()
}

fn provider_cost_catalog_operator_id(request_context: &AdminRequestContext<'_>) -> String {
    request_context
        .decision()
        .and_then(|decision| decision.admin_principal.as_ref())
        .map(|principal| principal.user_id.clone())
        .unwrap_or_else(|| "management_token".to_string())
}

fn provider_cost_catalog_id_from_path(request_path: &str) -> Option<String> {
    let value = request_path
        .strip_prefix(PROVIDER_COST_CATALOG_PATH)?
        .trim()
        .trim_matches('/')
        .to_string();
    if value.is_empty() || value.contains('/') {
        None
    } else {
        Some(value)
    }
}

fn parse_task_type(value: &str) -> Result<ProviderCostTaskType, Response<Body>> {
    ProviderCostTaskType::parse(value)
        .map_err(|_| build_admin_billing_bad_request_response("task_type must be text or image"))
}

fn parse_write_request(
    request_body: Option<&Bytes>,
) -> Result<ProviderCostCatalogWriteRequest, Response<Body>> {
    let body = request_body
        .filter(|body| !body.is_empty())
        .ok_or_else(|| {
            build_admin_billing_bad_request_response("request body must be a JSON object")
        })?;
    serde_json::from_slice(body)
        .map_err(|_| build_admin_billing_bad_request_response("request body is invalid"))
}

fn build_record(
    request: ProviderCostCatalogWriteRequest,
    cost_id: String,
    operator_id: String,
    now_unix_secs: u64,
    created_at_unix_secs: Option<u64>,
) -> Result<ProviderCostCatalogRecord, Response<Body>> {
    let task_type = parse_task_type(&request.task_type)?;
    let record = ProviderCostCatalogRecord {
        cost_id,
        provider_id: request.provider_id,
        model: request.model,
        task_type,
        currency: request.currency,
        price_per_request: request.price_per_request,
        tiered_pricing: request.tiered_pricing,
        effective_from_unix_secs: request.effective_from_unix_secs,
        effective_to_unix_secs: request.effective_to_unix_secs,
        created_by: operator_id,
        created_at_unix_secs: created_at_unix_secs.unwrap_or(now_unix_secs),
        updated_at_unix_secs: now_unix_secs,
    };
    record
        .validate()
        .map_err(|error| build_admin_billing_bad_request_response(error.to_string()))?;
    Ok(record)
}

pub(super) async fn maybe_build_local_admin_provider_cost_catalogs_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    match request_context.route_kind() {
        Some("list_provider_cost_catalogs") => Ok(Some(
            build_list_provider_cost_catalogs_response(state, request_context).await?,
        )),
        Some("get_provider_cost_catalog") => Ok(Some(
            build_get_provider_cost_catalog_response(state, request_context).await?,
        )),
        Some("find_effective_provider_cost_catalog") => Ok(Some(
            build_find_effective_provider_cost_catalog_response(state, request_context).await?,
        )),
        Some("create_provider_cost_catalog") => Ok(Some(
            build_create_provider_cost_catalog_response(state, request_context, request_body)
                .await?,
        )),
        Some("update_provider_cost_catalog") => Ok(Some(
            build_update_provider_cost_catalog_response(state, request_context, request_body)
                .await?,
        )),
        Some("delete_provider_cost_catalog") => Ok(Some(
            build_delete_provider_cost_catalog_response(state, request_context).await?,
        )),
        _ => Ok(None),
    }
}

async fn build_list_provider_cost_catalogs_response(
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
    let task_type = match admin_billing_optional_filter(request_context.query_string(), "task_type")
    {
        None => None,
        Some(value) => match parse_task_type(&value) {
            Ok(task_type) => Some(task_type),
            Err(response) => return Ok(response),
        },
    };
    let effective_at_unix_secs =
        match admin_billing_optional_filter(request_context.query_string(), "effective_at") {
            None => None,
            Some(value) => match value.parse::<u64>() {
                Ok(parsed) => Some(parsed),
                Err(_) => {
                    return Ok(build_admin_billing_bad_request_response(
                        "effective_at must be a unix timestamp",
                    ));
                }
            },
        };
    let query = ProviderCostCatalogListQuery {
        provider_id: admin_billing_optional_filter(request_context.query_string(), "provider_id"),
        model: admin_billing_optional_filter(request_context.query_string(), "model"),
        task_type,
        effective_at_unix_secs,
        limit: (page_size as usize).min(PROVIDER_COST_CATALOG_MAX_LIST_LIMIT),
        offset: page.saturating_sub(1).saturating_mul(page_size) as usize,
    };
    match state.app().list_provider_cost_catalogs(&query).await? {
        Some(items) => Ok(
            Json(json!({ "items": items, "page": page, "page_size": page_size })).into_response(),
        ),
        None => Ok(build_admin_billing_data_unavailable_response()),
    }
}

async fn build_get_provider_cost_catalog_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(cost_id) = provider_cost_catalog_id_from_path(request_context.path()) else {
        return Ok(build_admin_billing_bad_request_response(
            "provider cost catalog id is required",
        ));
    };
    match state.app().get_provider_cost_catalog(&cost_id).await? {
        Some(Some(record)) => Ok(Json(json!({ "item": record })).into_response()),
        Some(None) => Ok(build_admin_billing_not_found_response(
            "provider cost catalog not found",
        )),
        None => Ok(build_admin_billing_data_unavailable_response()),
    }
}

async fn build_find_effective_provider_cost_catalog_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let required = |key: &str| {
        crate::handlers::admin::shared::query_param_value(request_context.query_string(), key)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    let (Some(provider_id), Some(model), Some(task_type_value), Some(at_value)) = (
        required("provider_id"),
        required("model"),
        required("task_type"),
        required("at"),
    ) else {
        return Ok(build_admin_billing_bad_request_response(
            "provider_id, model, task_type and at are required",
        ));
    };
    let task_type = match parse_task_type(&task_type_value) {
        Ok(task_type) => task_type,
        Err(response) => return Ok(response),
    };
    let at_unix_secs = match at_value.parse::<u64>() {
        Ok(parsed) => parsed,
        Err(_) => {
            return Ok(build_admin_billing_bad_request_response(
                "at must be a unix timestamp",
            ));
        }
    };
    match state
        .app()
        .find_effective_provider_cost_catalog(&provider_id, &model, task_type, at_unix_secs)
        .await?
    {
        Some(Some(record)) => Ok(Json(json!({ "item": record })).into_response()),
        Some(None) => Ok(build_admin_billing_not_found_response(
            "no effective provider cost catalog for the given window",
        )),
        None => Ok(build_admin_billing_data_unavailable_response()),
    }
}

async fn build_create_provider_cost_catalog_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let request = match parse_write_request(request_body) {
        Ok(request) => request,
        Err(response) => return Ok(response),
    };
    let record = match build_record(
        request,
        uuid::Uuid::new_v4().to_string(),
        provider_cost_catalog_operator_id(request_context),
        now_unix_secs(),
        None,
    ) {
        Ok(record) => record,
        Err(response) => return Ok(response),
    };
    match state.app().upsert_provider_cost_catalog(record).await? {
        Some(outcome) => {
            let status = match outcome {
                ProviderCostCatalogUpsertOutcome::Inserted => "inserted",
                ProviderCostCatalogUpsertOutcome::Updated => "updated",
            };
            let response = Json(json!({ "outcome": status })).into_response();
            Ok(attach_admin_audit_response(
                response,
                "admin_provider_cost_catalog_saved",
                "create_provider_cost_catalog",
                "provider_cost_catalog",
                "catalog:count=1",
            ))
        }
        None => Ok(build_admin_billing_data_unavailable_response()),
    }
}

async fn build_update_provider_cost_catalog_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let Some(cost_id) = provider_cost_catalog_id_from_path(request_context.path()) else {
        return Ok(build_admin_billing_bad_request_response(
            "provider cost catalog id is required",
        ));
    };
    let existing = match state.app().get_provider_cost_catalog(&cost_id).await? {
        Some(Some(record)) => record,
        Some(None) => {
            return Ok(build_admin_billing_not_found_response(
                "provider cost catalog not found",
            ));
        }
        None => return Ok(build_admin_billing_data_unavailable_response()),
    };
    let request = match parse_write_request(request_body) {
        Ok(request) => request,
        Err(response) => return Ok(response),
    };
    let record = match build_record(
        request,
        cost_id.clone(),
        provider_cost_catalog_operator_id(request_context),
        now_unix_secs(),
        Some(existing.created_at_unix_secs),
    ) {
        Ok(record) => record,
        Err(response) => return Ok(response),
    };
    match state.app().upsert_provider_cost_catalog(record).await? {
        Some(_) => {
            let response =
                Json(json!({ "outcome": "updated", "cost_id": cost_id })).into_response();
            Ok(attach_admin_audit_response(
                response,
                "admin_provider_cost_catalog_saved",
                "update_provider_cost_catalog",
                "provider_cost_catalog",
                "catalog:count=1",
            ))
        }
        None => Ok(build_admin_billing_data_unavailable_response()),
    }
}

async fn build_delete_provider_cost_catalog_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(cost_id) = provider_cost_catalog_id_from_path(request_context.path()) else {
        return Ok(build_admin_billing_bad_request_response(
            "provider cost catalog id is required",
        ));
    };
    match state.app().delete_provider_cost_catalog(&cost_id).await? {
        Some(ProviderCostCatalogDeleteOutcome::Deleted) => {
            let response = Json(json!({ "deleted": true, "cost_id": cost_id })).into_response();
            Ok(attach_admin_audit_response(
                response,
                "admin_provider_cost_catalog_deleted",
                "delete_provider_cost_catalog",
                "provider_cost_catalog",
                "catalog:count=1",
            ))
        }
        Some(ProviderCostCatalogDeleteOutcome::NotFound) => Ok(
            build_admin_billing_not_found_response("provider cost catalog not found"),
        ),
        None => Ok(build_admin_billing_data_unavailable_response()),
    }
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
