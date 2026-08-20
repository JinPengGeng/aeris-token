use super::AdminAppState;

impl<'a> AdminAppState<'a> {
    pub(crate) async fn build_admin_health_summary_payload(&self) -> Option<serde_json::Value> {
        crate::handlers::admin::endpoint::build_admin_health_summary_payload(self).await
    }

    pub(crate) async fn build_admin_key_health_payload(
        &self,
        key_id: &str,
        api_format: Option<&str>,
    ) -> Option<serde_json::Value> {
        crate::handlers::admin::endpoint::build_admin_key_health_payload(self, key_id, api_format)
            .await
    }

    pub(crate) async fn recover_admin_key_health(
        &self,
        key_id: &str,
        api_format: Option<&str>,
    ) -> Option<serde_json::Value> {
        crate::handlers::admin::endpoint::recover_admin_key_health(self, key_id, api_format).await
    }

    pub(crate) async fn recover_all_admin_key_health(&self) -> Option<serde_json::Value> {
        crate::handlers::admin::endpoint::recover_all_admin_key_health(self).await
    }

    pub(crate) async fn repair_admin_half_open_isolation(
        &self,
        key_id: &str,
        api_format: &str,
        expected_fence: u64,
        expected_owner: &str,
    ) -> Result<crate::handlers::admin::endpoint::HalfOpenIsolationRepair, crate::GatewayError>
    {
        crate::handlers::admin::endpoint::repair_admin_half_open_isolation(
            self,
            key_id,
            api_format,
            expected_fence,
            expected_owner,
        )
        .await
    }

    pub(crate) async fn build_admin_endpoint_health_status_payload(
        &self,
        lookback_hours: u64,
    ) -> Option<serde_json::Value> {
        crate::handlers::admin::endpoint::build_admin_endpoint_health_status_payload(
            self,
            lookback_hours,
        )
        .await
    }

    pub(crate) async fn build_admin_key_rpm_payload(
        &self,
        key_id: &str,
    ) -> Option<serde_json::Value> {
        crate::handlers::admin::endpoint::build_admin_key_rpm_payload(self, key_id).await
    }
}
