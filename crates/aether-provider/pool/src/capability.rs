#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderPoolCapability {
    PlanTier,
    QuotaReset,
    QuotaRefresh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProviderPoolCapabilities {
    pub plan_tier: bool,
    pub quota_reset: bool,
    pub quota_refresh: bool,
}

impl ProviderPoolCapabilities {
    pub fn for_builtin_provider(provider_type: &str) -> Self {
        Self {
            quota_refresh:
                aether_provider_transport::provider_types::provider_type_supports_quota_refresh(
                    provider_type,
                ),
            ..Self::default()
        }
    }

    pub fn supports(self, capability: ProviderPoolCapability) -> bool {
        match capability {
            ProviderPoolCapability::PlanTier => self.plan_tier,
            ProviderPoolCapability::QuotaReset => self.quota_reset,
            ProviderPoolCapability::QuotaRefresh => self.quota_refresh,
        }
    }
}
