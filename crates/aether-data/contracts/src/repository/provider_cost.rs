use async_trait::async_trait;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCostDimension {
    Input,
    Output,
    CacheRead,
    CacheWrite,
    Image,
    Request,
}

impl ProviderCostDimension {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::CacheRead => "cache_read",
            Self::CacheWrite => "cache_write",
            Self::Image => "image",
            Self::Request => "request",
        }
    }

    pub fn parse(value: &str) -> Result<Self, crate::DataLayerError> {
        match value {
            "input" => Ok(Self::Input),
            "output" => Ok(Self::Output),
            "cache_read" => Ok(Self::CacheRead),
            "cache_write" => Ok(Self::CacheWrite),
            "image" => Ok(Self::Image),
            "request" => Ok(Self::Request),
            _ => Err(crate::DataLayerError::UnexpectedValue(format!(
                "unknown provider cost dimension: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCostUnit {
    PerMillionTokens,
    PerImage,
    PerRequest,
}

impl ProviderCostUnit {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PerMillionTokens => "per_million_tokens",
            Self::PerImage => "per_image",
            Self::PerRequest => "per_request",
        }
    }

    pub fn parse(value: &str) -> Result<Self, crate::DataLayerError> {
        match value {
            "per_million_tokens" => Ok(Self::PerMillionTokens),
            "per_image" => Ok(Self::PerImage),
            "per_request" => Ok(Self::PerRequest),
            _ => Err(crate::DataLayerError::UnexpectedValue(format!(
                "unknown provider cost unit: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCostCertainty {
    Known,
    Estimated,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCostSourceKind {
    SupplierBill,
    ManualImport,
    Estimate,
}

impl ProviderCostSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SupplierBill => "supplier_bill",
            Self::ManualImport => "manual_import",
            Self::Estimate => "estimate",
        }
    }

    pub fn parse(value: &str) -> Result<Self, crate::DataLayerError> {
        match value {
            "supplier_bill" => Ok(Self::SupplierBill),
            "manual_import" => Ok(Self::ManualImport),
            "estimate" => Ok(Self::Estimate),
            _ => Err(crate::DataLayerError::UnexpectedValue(format!(
                "unknown provider cost source kind: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCostReconciliationStatus {
    Unreconciled,
    Matched,
    Disputed,
    NotApplicable,
}

impl ProviderCostReconciliationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unreconciled => "unreconciled",
            Self::Matched => "matched",
            Self::Disputed => "disputed",
            Self::NotApplicable => "not_applicable",
        }
    }

    pub fn parse(value: &str) -> Result<Self, crate::DataLayerError> {
        match value {
            "unreconciled" => Ok(Self::Unreconciled),
            "matched" => Ok(Self::Matched),
            "disputed" => Ok(Self::Disputed),
            "not_applicable" => Ok(Self::NotApplicable),
            _ => Err(crate::DataLayerError::UnexpectedValue(format!(
                "unknown provider cost reconciliation status: {value}"
            ))),
        }
    }
}

impl ProviderCostCertainty {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Known => "known",
            Self::Estimated => "estimated",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(value: &str) -> Result<Self, crate::DataLayerError> {
        match value {
            "known" => Ok(Self::Known),
            "estimated" => Ok(Self::Estimated),
            "unknown" => Ok(Self::Unknown),
            _ => Err(crate::DataLayerError::UnexpectedValue(format!(
                "unknown provider cost certainty: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProviderCostPrice {
    pub import_id: String,
    pub supplier: String,
    pub provider: String,
    pub model: String,
    pub dimension: ProviderCostDimension,
    pub currency: String,
    pub unit: ProviderCostUnit,
    pub version: String,
    pub price_units: u64,
    pub effective_from_unix_secs: u64,
    pub effective_to_unix_secs: Option<u64>,
    pub source_reference: String,
    pub imported_by: String,
}

impl ProviderCostPrice {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        for (name, value) in [
            ("import_id", self.import_id.as_str()),
            ("supplier", self.supplier.as_str()),
            ("provider", self.provider.as_str()),
            ("model", self.model.as_str()),
            ("currency", self.currency.as_str()),
            ("version", self.version.as_str()),
            ("source_reference", self.source_reference.as_str()),
            ("imported_by", self.imported_by.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(crate::DataLayerError::InvalidInput(format!(
                    "provider cost price {name} is empty"
                )));
            }
        }
        if self
            .effective_to_unix_secs
            .is_some_and(|end| end <= self.effective_from_unix_secs)
        {
            return Err(crate::DataLayerError::InvalidInput(
                "provider cost price effective window is invalid".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProviderCostPriceComponent {
    pub dimension: ProviderCostDimension,
    pub quantity: u64,
    pub unit: ProviderCostUnit,
    pub price_import_id: String,
    pub price_version: String,
    pub price_source_reference: String,
    pub amount_units: u64,
}

impl ProviderCostPriceComponent {
    fn validate(&self) -> Result<(), crate::DataLayerError> {
        for (name, value) in [
            ("price_import_id", self.price_import_id.as_str()),
            ("price_version", self.price_version.as_str()),
            (
                "price_source_reference",
                self.price_source_reference.as_str(),
            ),
        ] {
            if value.trim().is_empty() {
                return Err(crate::DataLayerError::InvalidInput(format!(
                    "provider cost price component {name} is empty"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProviderCostSnapshotImport {
    pub import_id: String,
    pub request_id: String,
    pub provider: String,
    pub model: String,
    pub dimension: ProviderCostDimension,
    pub sales_amount_units: u64,
    pub sales_currency: String,
    pub provider_cost_amount_units: Option<u64>,
    pub provider_currency: Option<String>,
    pub certainty: ProviderCostCertainty,
    pub source_kind: ProviderCostSourceKind,
    pub reconciliation_status: ProviderCostReconciliationStatus,
    pub price_import_id: Option<String>,
    pub price_version: Option<String>,
    pub source_reference: Option<String>,
    #[serde(default)]
    pub price_components: Vec<ProviderCostPriceComponent>,
    pub occurred_at_unix_secs: u64,
    pub imported_by: String,
}

impl ProviderCostSnapshotImport {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        for (name, value) in [
            ("import_id", self.import_id.as_str()),
            ("request_id", self.request_id.as_str()),
            ("provider", self.provider.as_str()),
            ("model", self.model.as_str()),
            ("sales_currency", self.sales_currency.as_str()),
            ("imported_by", self.imported_by.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(crate::DataLayerError::InvalidInput(format!(
                    "provider cost snapshot {name} is empty"
                )));
            }
        }
        let has_amount_and_currency = self.provider_cost_amount_units.is_some()
            && self
                .provider_currency
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty());
        let has_components = !self.price_components.is_empty();
        let mut dimensions = std::collections::BTreeSet::new();
        for component in &self.price_components {
            component.validate()?;
            if !dimensions.insert(component.dimension.as_str()) {
                return Err(crate::DataLayerError::InvalidInput(
                    "provider cost price components must have unique dimensions".into(),
                ));
            }
        }
        match self.certainty {
            ProviderCostCertainty::Unknown
                if self.provider_cost_amount_units.is_some()
                    || self.provider_currency.is_some()
                    || self.price_import_id.is_some()
                    || self.price_version.is_some()
                    || has_components =>
            {
                Err(crate::DataLayerError::InvalidInput(
                    "unknown provider cost must not contain amount, currency, or price reference"
                        .into(),
                ))
            }
            ProviderCostCertainty::Estimated
                if !has_amount_and_currency
                    || self
                        .source_reference
                        .as_deref()
                        .is_none_or(|value| value.trim().is_empty())
                    || (!has_components
                        && (self
                            .price_import_id
                            .as_deref()
                            .is_none_or(|value| value.trim().is_empty())
                            || self
                                .price_version
                                .as_deref()
                                .is_none_or(|value| value.trim().is_empty())))
                    || (has_components && self.dimension != ProviderCostDimension::Request) =>
            {
                Err(crate::DataLayerError::InvalidInput(
                    "estimated provider cost requires amount, currency, source, and either a price reference or request components"
                        .into(),
                ))
            }
            ProviderCostCertainty::Estimated
                if self.source_kind != ProviderCostSourceKind::Estimate =>
            {
                Err(crate::DataLayerError::InvalidInput(
                    "estimated provider cost requires estimate source kind".into(),
                ))
            }
            ProviderCostCertainty::Known
                if !has_amount_and_currency
                    || self
                        .source_reference
                        .as_deref()
                        .is_none_or(|value| value.trim().is_empty()) =>
            {
                Err(crate::DataLayerError::InvalidInput(
                    "known provider cost requires amount, currency, and source".into(),
                ))
            }
            ProviderCostCertainty::Known
                if self.source_kind != ProviderCostSourceKind::SupplierBill =>
            {
                Err(crate::DataLayerError::InvalidInput(
                    "known provider cost requires supplier_bill source kind".into(),
                ))
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredProviderCostSnapshot {
    pub import: ProviderCostSnapshotImport,
    pub imported_at_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProviderCostImportOutcome<T> {
    pub record: T,
    pub inserted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProviderCostListQuery {
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProviderCostSummaryQuery {
    pub occurred_from_unix_secs: u64,
    pub occurred_until_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredProviderCostSummaryRow {
    pub sales_currency: String,
    pub provider_currency: Option<String>,
    pub certainty: ProviderCostCertainty,
    pub source_kind: ProviderCostSourceKind,
    pub reconciliation_status: ProviderCostReconciliationStatus,
    pub price_version: Option<String>,
    pub sales_amount_units: u64,
    pub provider_cost_amount_units: Option<u64>,
    pub margin_amount_units: Option<i64>,
    pub snapshot_count: u64,
    pub unreconciled_count: u64,
    pub unknown_count: u64,
}

#[async_trait]
pub trait ProviderCostRepository: Send + Sync {
    async fn import_price(
        &self,
        price: &ProviderCostPrice,
    ) -> Result<ProviderCostImportOutcome<ProviderCostPrice>, crate::DataLayerError>;

    #[allow(clippy::too_many_arguments)]
    async fn find_effective_price(
        &self,
        supplier: &str,
        provider: &str,
        model: &str,
        dimension: ProviderCostDimension,
        currency: &str,
        unit: ProviderCostUnit,
        at_unix_secs: u64,
    ) -> Result<Option<ProviderCostPrice>, crate::DataLayerError>;

    async fn list_prices(
        &self,
        query: &ProviderCostListQuery,
    ) -> Result<Vec<ProviderCostPrice>, crate::DataLayerError>;

    async fn import_snapshot(
        &self,
        snapshot: &ProviderCostSnapshotImport,
    ) -> Result<ProviderCostImportOutcome<StoredProviderCostSnapshot>, crate::DataLayerError>;

    async fn list_snapshots_for_request(
        &self,
        request_id: &str,
    ) -> Result<Vec<StoredProviderCostSnapshot>, crate::DataLayerError>;

    async fn summarize_snapshots(
        &self,
        query: &ProviderCostSummaryQuery,
    ) -> Result<Vec<StoredProviderCostSummaryRow>, crate::DataLayerError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_snapshot_requires_null_provider_cost_fields() {
        let mut input = ProviderCostSnapshotImport {
            import_id: "cost-1".into(),
            request_id: "request-1".into(),
            provider: "provider-1".into(),
            model: "model-1".into(),
            dimension: ProviderCostDimension::Input,
            sales_amount_units: 10,
            sales_currency: "USD".into(),
            provider_cost_amount_units: None,
            provider_currency: None,
            certainty: ProviderCostCertainty::Unknown,
            source_kind: ProviderCostSourceKind::ManualImport,
            reconciliation_status: ProviderCostReconciliationStatus::Unreconciled,
            price_import_id: None,
            price_version: None,
            source_reference: None,
            price_components: Vec::new(),
            occurred_at_unix_secs: 100,
            imported_by: "admin-1".into(),
        };
        assert!(input.validate().is_ok());
        input.provider_cost_amount_units = Some(0);
        assert!(input.validate().is_err());
    }

    #[test]
    fn known_snapshot_rejects_estimate_source() {
        let input = ProviderCostSnapshotImport {
            import_id: "cost-known".into(),
            request_id: "request-known".into(),
            provider: "provider-1".into(),
            model: "model-1".into(),
            dimension: ProviderCostDimension::Input,
            sales_amount_units: 10,
            sales_currency: "USD".into(),
            provider_cost_amount_units: Some(5),
            provider_currency: Some("USD".into()),
            certainty: ProviderCostCertainty::Known,
            source_kind: ProviderCostSourceKind::Estimate,
            reconciliation_status: ProviderCostReconciliationStatus::Unreconciled,
            price_import_id: None,
            price_version: None,
            source_reference: Some("synthetic-source".into()),
            price_components: Vec::new(),
            occurred_at_unix_secs: 100,
            imported_by: "admin-1".into(),
        };
        assert!(input.validate().is_err());
    }
}
