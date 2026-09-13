//! Emit the real billing/logging metric families for the Prometheus parser.
use aether_runtime::{
    logging_metric_samples, record_billing_enrichment_failure,
    record_billing_fail_open_daily_quota, record_billing_fail_open_rpm,
    record_billing_settlement_failure, record_video_task_settlement_failure, ServiceRuntimeConfig,
};

fn main() {
    aether_runtime::metrics::init_metrics(
        ServiceRuntimeConfig::new("gateway", "warn").with_metrics_namespace("aether_gateway"),
    );
    // A fresh process provides the zero baseline for the real scrape/alert drill.
    // Default output retains the existing failure fixture used by promtool CI.
    if !std::env::args().any(|arg| arg == "--healthy") {
        record_billing_enrichment_failure();
        record_billing_settlement_failure();
        record_video_task_settlement_failure();
        record_billing_fail_open_daily_quota();
        record_billing_fail_open_rpm();
    }
    print!(
        "{}",
        aether_runtime::metrics::render_prometheus_text(&logging_metric_samples())
    );
}
