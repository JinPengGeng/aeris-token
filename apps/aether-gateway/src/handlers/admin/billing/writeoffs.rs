use super::build_admin_billing_bad_request_response;
use crate::app_timezone::{app_timezone, local_day_window};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::query_param_value;
use crate::GatewayError;
use aether_data_contracts::repository::usage::{
    InsufficientQuotaWriteoffQuery, INSUFFICIENT_QUOTA_WRITEOFF_MAX_LIMIT,
};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use chrono::{Duration, Utc};
use serde_json::json;

/// Read-only daily writeoff report: usage finalized as `insufficient_quota`.
///
/// Query params (both optional, unix seconds): `from`, `to`. Defaults to the
/// last complete local application day.
pub(super) async fn maybe_build_local_admin_insufficient_quota_writeoffs_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };
    if decision.route_kind.as_deref() != Some("list_insufficient_quota_writeoffs")
        || request_context.method() != http::Method::GET
    {
        return Ok(None);
    }
    let path = request_context.path();
    if !matches!(
        path,
        "/api/admin/billing/insufficient-quota-writeoffs"
            | "/api/admin/billing/insufficient-quota-writeoffs/"
    ) {
        return Ok(None);
    }

    Ok(Some(
        build_admin_insufficient_quota_writeoffs_response(state, request_context.query_string())
            .await?,
    ))
}

async fn build_admin_insufficient_quota_writeoffs_response(
    state: &AdminAppState<'_>,
    query: Option<&str>,
) -> Result<Response<Body>, GatewayError> {
    let query = match parse_writeoff_query(query) {
        Ok(query) => query,
        Err(detail) => return Ok(build_admin_billing_bad_request_response(detail)),
    };
    let items = state
        .app()
        .data
        .list_insufficient_quota_writeoffs(&query)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;

    let total_writeoff_usd: f64 = items.iter().map(|item| item.total_cost_usd).sum();
    Ok(Json(json!({
        "items": items,
        "total": items.len(),
        "total_writeoff_usd": total_writeoff_usd,
        "finalized_from_unix_secs": query.finalized_from_unix_secs,
        "finalized_until_unix_secs": query.finalized_until_unix_secs,
        "limit": query.limit,
    }))
    .into_response())
}

fn parse_writeoff_query(query: Option<&str>) -> Result<InsufficientQuotaWriteoffQuery, String> {
    let from = parse_unix_secs(query, "from")?;
    let to = parse_unix_secs(query, "to")?;
    let limit = match query_param_value(query, "limit") {
        None => INSUFFICIENT_QUOTA_WRITEOFF_MAX_LIMIT,
        Some(value) => value.parse::<usize>().map_err(|_| {
            format!("limit must be between 1 and {INSUFFICIENT_QUOTA_WRITEOFF_MAX_LIMIT}")
        })?,
    };
    let (from, to) = match (from, to) {
        (Some(from), Some(to)) => (from, to),
        (None, None) => {
            let (_, day_start, day_end) =
                local_day_window(Utc::now() - Duration::days(1), app_timezone());
            (
                day_start.timestamp().max(0) as u64,
                day_end.timestamp().max(0) as u64,
            )
        }
        _ => {
            return Err("from and to must be provided together".to_string());
        }
    };
    let query = InsufficientQuotaWriteoffQuery {
        finalized_from_unix_secs: from,
        finalized_until_unix_secs: to,
        limit,
    };
    query.validate().map_err(|err| err.to_string())?;
    Ok(query)
}

fn parse_unix_secs(query: Option<&str>, key: &str) -> Result<Option<u64>, String> {
    match query_param_value(query, key) {
        None => Ok(None),
        Some(value) => value
            .parse::<u64>()
            .map(Some)
            .map_err(|_| format!("{key} must be a unix timestamp in seconds")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_window_and_limit_parse() {
        let query =
            parse_writeoff_query(Some("from=100&to=200&limit=50")).expect("query should parse");
        assert_eq!(query.finalized_from_unix_secs, 100);
        assert_eq!(query.finalized_until_unix_secs, 200);
        assert_eq!(query.limit, 50);
    }

    #[test]
    fn default_window_is_the_last_complete_local_day() {
        let query = parse_writeoff_query(None).expect("query should parse");
        assert!(query.finalized_from_unix_secs < query.finalized_until_unix_secs);
        assert_eq!(query.limit, INSUFFICIENT_QUOTA_WRITEOFF_MAX_LIMIT);
    }

    #[test]
    fn invalid_params_are_rejected() {
        assert!(parse_writeoff_query(Some("from=100")).is_err());
        assert!(parse_writeoff_query(Some("from=abc&to=200")).is_err());
        assert!(parse_writeoff_query(Some("from=200&to=100")).is_err());
        assert!(parse_writeoff_query(Some(&format!(
            "limit={}",
            INSUFFICIENT_QUOTA_WRITEOFF_MAX_LIMIT + 1
        )))
        .is_err());
    }
}
