use super::super::super::stats::resolve_admin_usage_time_range;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::query_param_value;
use crate::GatewayError;
use aether_admin::observability::stats::round_to;
use aether_admin::observability::usage::{
    admin_usage_bad_request_response, admin_usage_data_unavailable_response,
    admin_usage_parse_aggregation_limit, ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
};
use aether_data_contracts::repository::usage::{
    MarginReportGranularity, MarginReportQuery, StoredMarginReportRow,
};
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

fn clamp_units(units: i128) -> i64 {
    units.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

fn margin_report_row_json(row: &StoredMarginReportRow) -> serde_json::Value {
    let revenue_units = clamp_units(row.revenue_units);
    let cost_units = clamp_units(row.cost_units);
    // Rows carrying any unknown-cost attempt must not fake a zero cost basis:
    // report margin as unknown (null) instead of revenue minus partial cost.
    let (margin, margin_rate) = if row.cost_unknown_request_count > 0 {
        (serde_json::Value::Null, serde_json::Value::Null)
    } else {
        let margin_units = clamp_units(row.revenue_units - row.cost_units);
        let rate = if revenue_units == 0 {
            0.0
        } else {
            round_to(margin_units as f64 / revenue_units as f64 * 100.0, 2)
        };
        (
            json!(crate::money_fixed::format_money_units(margin_units)),
            json!(rate),
        )
    };
    let estimated_coverage = if row.request_count == 0 {
        0.0
    } else {
        round_to(
            row.cost_estimated_request_count as f64 / row.request_count as f64 * 100.0,
            2,
        )
    };
    json!({
        "period_start": row.period_start,
        "model": row.model,
        "provider_id": row.provider_id,
        "request_count": row.request_count,
        "revenue": crate::money_fixed::format_money_units(revenue_units),
        "cost": crate::money_fixed::format_money_units(cost_units),
        "margin": margin,
        "margin_rate": margin_rate,
        "currency": row.currency,
        "cost_coverage": {
            "estimated_requests": row.cost_estimated_request_count,
            "known_requests": row.cost_known_request_count,
            "unknown_requests": row.cost_unknown_request_count,
            "total_requests": row.request_count,
            "estimated_share_percent": estimated_coverage,
        },
    })
}

pub(super) async fn build_admin_usage_margin_stats_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_usage_data_reader() {
        return Ok(admin_usage_data_unavailable_response(
            ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
        ));
    }

    let query = request_context.request_query_string.as_deref();
    let granularity = query_param_value(query, "granularity")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let granularity = match granularity.as_str() {
        "" | "day" => MarginReportGranularity::Day,
        "week" => MarginReportGranularity::Week,
        "month" => MarginReportGranularity::Month,
        _ => {
            return Ok(admin_usage_bad_request_response(
                "Invalid granularity value: must be one of day, week, month",
            ));
        }
    };
    let limit = match admin_usage_parse_aggregation_limit(query) {
        Ok(value) => value,
        Err(detail) => return Ok(admin_usage_bad_request_response(detail)),
    };
    let time_range = match resolve_admin_usage_time_range(query) {
        Ok(value) => value,
        Err(detail) => return Ok(admin_usage_bad_request_response(detail)),
    };
    let Some((created_from_unix_secs, created_until_unix_secs)) = time_range.to_unix_bounds()
    else {
        return Ok(Json(json!([])).into_response());
    };
    let provider_id = query_param_value(query, "provider_id")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let model = query_param_value(query, "model")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    let rows = state
        .aggregate_margin_report(&MarginReportQuery {
            created_from_unix_secs,
            created_until_unix_secs,
            granularity,
            provider_id,
            model,
            limit,
        })
        .await?;

    Ok(Json(json!(rows
        .iter()
        .map(margin_report_row_json)
        .collect::<Vec<_>>()))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        revenue_units: i128,
        cost_units: i128,
        cost_estimated_request_count: u64,
        cost_known_request_count: u64,
        cost_unknown_request_count: u64,
    ) -> StoredMarginReportRow {
        StoredMarginReportRow {
            period_start: "2024-03-21".to_string(),
            model: "gpt-5".to_string(),
            provider_id: "provider-openai".to_string(),
            request_count: 10,
            revenue_units,
            cost_units,
            cost_known_request_count,
            cost_estimated_request_count,
            cost_unknown_request_count,
            currency: Some("USD".to_string()),
        }
    }

    #[test]
    fn margin_row_computes_fixed_point_margin_and_rate() {
        let value = margin_report_row_json(&row(200_000_000, 150_000_000, 10, 0, 0));
        assert_eq!(value["revenue"], "2.00000000");
        assert_eq!(value["cost"], "1.50000000");
        assert_eq!(value["margin"], "0.50000000");
        assert_eq!(value["margin_rate"], 25.0);
        assert_eq!(value["cost_coverage"]["estimated_requests"], 10);
        assert_eq!(value["cost_coverage"]["estimated_share_percent"], 100.0);
    }

    #[test]
    fn margin_row_marks_margin_unknown_when_cost_coverage_unknown() {
        let value = margin_report_row_json(&row(200_000_000, 100_000_000, 5, 0, 5));
        assert_eq!(value["margin"], serde_json::Value::Null);
        assert_eq!(value["margin_rate"], serde_json::Value::Null);
        assert_eq!(value["cost_coverage"]["unknown_requests"], 5);
        assert_eq!(value["cost_coverage"]["estimated_share_percent"], 50.0);
    }
}
