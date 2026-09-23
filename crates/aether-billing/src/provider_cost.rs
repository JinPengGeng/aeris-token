use serde::{Deserialize, Serialize};

pub use aether_data_contracts::repository::provider_cost::{
    ProviderCostCertainty as CostCertainty, ProviderCostDimension, ProviderCostPrice,
    ProviderCostUnit,
};

/// Supplier money uses the existing billing storage precision as integer currency units.
pub const PROVIDER_COST_SCALE: u32 = crate::precision::BILLING_STORAGE_PRECISION;
const TOKENS_PER_MILLION: u128 = 1_000_000;

/// Declares whether an input price excludes cache components or represents a
/// supplier's inclusive total input. The caller must supply this supplier
/// price-book semantic; it cannot be inferred from observed usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCostInputPriceMode {
    /// Variant: exclusive of cache.
    ExclusiveOfCache,
    /// Variant: inclusive of cache.
    InclusiveOfCache,
}

/// Token quantities mapped from one standardized usage record for supplier
/// pricing. Each quantity can be paired with only its matching price dimension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCostTokenQuantities {
    /// Field: input.
    pub input: u64,
    /// Field: output.
    pub output: u64,
    /// Field: cache write.
    pub cache_write: u64,
    /// Field: cache read.
    pub cache_read: u64,
}

/// Maps standardized token usage into mutually exclusive provider-cost
/// quantities. It preserves an unknown estimate as `None` for malformed token
/// values, cache-breakdown conflicts, unknown API cache semantics, or an
/// inclusive input price accompanied by separately billable cache components.
pub fn provider_cost_token_quantities_from_standardized_usage(
    api_format: Option<&str>,
    usage: &crate::StandardizedUsage,
    input_price_mode: ProviderCostInputPriceMode,
) -> Option<ProviderCostTokenQuantities> {
    let [input_tokens, output_tokens, cache_creation_tokens, cache_creation_5m_tokens, cache_creation_1h_tokens, cache_read_tokens] = [
        usage.input_tokens,
        usage.output_tokens,
        usage.cache_creation_tokens,
        usage.cache_creation_ephemeral_5m_tokens,
        usage.cache_creation_ephemeral_1h_tokens,
        usage.cache_read_tokens,
    ];
    if [
        input_tokens,
        output_tokens,
        cache_creation_tokens,
        cache_creation_5m_tokens,
        cache_creation_1h_tokens,
        cache_read_tokens,
    ]
    .into_iter()
    .any(|value| value < 0)
    {
        return None;
    }

    let classified_cache_creation =
        cache_creation_5m_tokens.checked_add(cache_creation_1h_tokens)?;
    let cache_write_tokens = match cache_creation_tokens {
        0 => classified_cache_creation,
        aggregate if classified_cache_creation <= aggregate => aggregate,
        _ => return None,
    };
    let has_cache_components = cache_write_tokens > 0 || cache_read_tokens > 0;
    if input_price_mode == ProviderCostInputPriceMode::InclusiveOfCache && has_cache_components {
        return None;
    }

    let normalized_input_tokens = if has_cache_components {
        crate::normalize_input_tokens_for_billing_with_known_cache_semantics(
            api_format,
            input_tokens,
            cache_write_tokens,
            cache_read_tokens,
        )?
    } else {
        crate::normalize_input_tokens_for_billing(api_format, input_tokens, 0, 0)
    };

    Some(ProviderCostTokenQuantities {
        input: u64::try_from(normalized_input_tokens).ok()?,
        output: u64::try_from(output_tokens).ok()?,
        cache_write: u64::try_from(cache_write_tokens).ok()?,
        cache_read: u64::try_from(cache_read_tokens).ok()?,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Data type: provider cost estimate input.
pub struct ProviderCostEstimateInput {
    /// Field: price.
    pub price: Option<ProviderCostPrice>,
    /// Field: quantity.
    pub quantity: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Data type: provider cost estimate component.
pub struct ProviderCostEstimateComponent {
    /// Field: dimension.
    pub dimension: ProviderCostDimension,
    /// Field: quantity.
    pub quantity: u64,
    /// Field: unit.
    pub unit: ProviderCostUnit,
    /// Field: price import id.
    pub price_import_id: String,
    /// Field: price version.
    pub price_version: String,
    /// Field: price source reference.
    pub price_source_reference: String,
    /// Field: amount units.
    pub amount_units: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Data type: provider cost request estimate.
pub struct ProviderCostRequestEstimate {
    /// Field: currency.
    pub currency: String,
    /// Field: amount units.
    pub amount_units: u64,
    /// Field: components.
    pub components: Vec<ProviderCostEstimateComponent>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
/// Enumeration: provider cost estimate error.
pub enum ProviderCostEstimateError {
    #[error(transparent)]
    /// Variant: invalid price.
    InvalidPrice(#[from] ProviderCostPriceError),
    #[error("provider cost unit {unit:?} does not match dimension {dimension:?}")]
    /// Variant: unit dimension mismatch.
    UnitDimensionMismatch {
        /// Field: unit.
        unit: ProviderCostUnit,
        /// Field: dimension.
        dimension: ProviderCostDimension,
    },
    #[error("provider cost unit {0:?} is not supported for automatic estimates")]
    /// Variant: unsupported unit.
    UnsupportedUnit(ProviderCostUnit),
    #[error("provider cost estimate components use different currencies")]
    /// Variant: mixed currencies.
    MixedCurrencies,
    #[error(
        "provider cost estimate components use different supplier, provider, or model identities"
    )]
    /// Variant: mixed price identity.
    MixedPriceIdentity,
    #[error("provider cost estimate repeats dimension {0:?}")]
    /// Variant: duplicate dimension.
    DuplicateDimension(ProviderCostDimension),
    #[error("provider cost estimate arithmetic overflowed")]
    /// Variant: arithmetic overflow.
    ArithmeticOverflow,
}

fn provider_cost_unit_denominator(
    dimension: ProviderCostDimension,
    unit: ProviderCostUnit,
) -> Result<u128, ProviderCostEstimateError> {
    match (dimension, unit) {
        (
            ProviderCostDimension::Input
            | ProviderCostDimension::Output
            | ProviderCostDimension::CacheRead
            | ProviderCostDimension::CacheWrite,
            ProviderCostUnit::PerMillionTokens,
        ) => Ok(TOKENS_PER_MILLION),
        (ProviderCostDimension::Request, ProviderCostUnit::PerRequest) => Ok(1),
        (ProviderCostDimension::Image, ProviderCostUnit::PerImage) => Ok(1),
        (_, ProviderCostUnit::PerImage) => Err(ProviderCostEstimateError::UnsupportedUnit(unit)),
        _ => Err(ProviderCostEstimateError::UnitDimensionMismatch { unit, dimension }),
    }
}

/// Calculates a supplier amount in fixed-point currency units, rounding up to
/// the smallest representable unit whenever a fractional unit remains.
pub fn calculate_provider_cost_amount(
    quantity: u64,
    price_units: u64,
    unit: ProviderCostUnit,
    dimension: ProviderCostDimension,
) -> Result<u64, ProviderCostEstimateError> {
    let denominator = provider_cost_unit_denominator(dimension, unit)?;
    if quantity == 0 || price_units == 0 {
        return Ok(0);
    }
    let numerator = u128::from(quantity)
        .checked_mul(u128::from(price_units))
        .ok_or(ProviderCostEstimateError::ArithmeticOverflow)?;
    let rounded = numerator
        .checked_div(denominator)
        .and_then(|whole| whole.checked_add(u128::from((numerator % denominator != 0) as u8)))
        .ok_or(ProviderCostEstimateError::ArithmeticOverflow)?;
    u64::try_from(rounded).map_err(|_| ProviderCostEstimateError::ArithmeticOverflow)
}

/// Function: estimate provider cost component.
pub fn estimate_provider_cost_component(
    price: &ProviderCostPrice,
    quantity: u64,
) -> Result<ProviderCostEstimateComponent, ProviderCostEstimateError> {
    validate_price(price)?;
    Ok(ProviderCostEstimateComponent {
        dimension: price.dimension,
        quantity,
        unit: price.unit,
        price_import_id: price.import_id.clone(),
        price_version: price.version.clone(),
        price_source_reference: price.source_reference.clone(),
        amount_units: calculate_provider_cost_amount(
            quantity,
            price.price_units,
            price.unit,
            price.dimension,
        )?,
    })
}

/// Resolves a settled attempt's provider cost from a supplier price book.
/// Every price is resolved at `frozen_at_unix_secs` — the reservation's frozen
/// admission time — so catalog changes that take effect later never rewrite an
/// already settled attempt. Token dimensions are required whenever quantities
/// are supplied; cache dimensions join only for positive frozen quantities and
/// the per-image dimension only for image attempts. Returns `Ok(None)` when a
/// required price is absent at the frozen time, preserving unknown cost.
#[allow(clippy::too_many_arguments)]
pub fn estimate_attempt_provider_cost(
    prices: &[ProviderCostPrice],
    supplier: &str,
    provider: &str,
    model: &str,
    quantities: Option<&ProviderCostTokenQuantities>,
    image_count: u64,
    currency: &str,
    frozen_at_unix_secs: u64,
) -> Result<Option<ProviderCostRequestEstimate>, ProviderCostEstimateError> {
    if quantities.is_none() && image_count == 0 {
        return Ok(None);
    }
    let mut inputs = Vec::with_capacity(5);
    if let Some(quantities) = quantities {
        for (dimension, quantity) in [
            (ProviderCostDimension::Input, quantities.input),
            (ProviderCostDimension::Output, quantities.output),
        ] {
            inputs.push(ProviderCostEstimateInput {
                price: resolve_provider_cost_price(
                    prices,
                    supplier,
                    provider,
                    model,
                    dimension,
                    currency,
                    ProviderCostUnit::PerMillionTokens,
                    frozen_at_unix_secs,
                )
                .ok()
                .cloned(),
                quantity: Some(quantity),
            });
        }
        for (dimension, quantity) in [
            (ProviderCostDimension::CacheWrite, quantities.cache_write),
            (ProviderCostDimension::CacheRead, quantities.cache_read),
        ] {
            if quantity == 0 {
                continue;
            }
            inputs.push(ProviderCostEstimateInput {
                price: resolve_provider_cost_price(
                    prices,
                    supplier,
                    provider,
                    model,
                    dimension,
                    currency,
                    ProviderCostUnit::PerMillionTokens,
                    frozen_at_unix_secs,
                )
                .ok()
                .cloned(),
                quantity: Some(quantity),
            });
        }
    }
    if image_count > 0 {
        inputs.push(ProviderCostEstimateInput {
            price: resolve_provider_cost_price(
                prices,
                supplier,
                provider,
                model,
                ProviderCostDimension::Image,
                currency,
                ProviderCostUnit::PerImage,
                frozen_at_unix_secs,
            )
            .ok()
            .cloned(),
            quantity: Some(image_count),
        });
    }
    estimate_provider_request_cost(&inputs)
}

/// Returns `None` when a component price or quantity is absent, preserving
/// unknown provider cost instead of estimating from incomplete observed usage.
/// Callers must select an effective price version before constructing inputs.
pub fn estimate_provider_request_cost(
    inputs: &[ProviderCostEstimateInput],
) -> Result<Option<ProviderCostRequestEstimate>, ProviderCostEstimateError> {
    if inputs.is_empty()
        || inputs
            .iter()
            .any(|input| input.price.is_none() || input.quantity.is_none())
    {
        return Ok(None);
    }

    let mut components = Vec::with_capacity(inputs.len());
    let mut currency: Option<&str> = None;
    let mut identity: Option<(&str, &str, &str)> = None;
    let mut amount_units = 0_u64;
    for input in inputs {
        let price = input
            .price
            .as_ref()
            .expect("missing prices returned unknown");
        let component = estimate_provider_cost_component(price, input.quantity.unwrap())?;
        match currency {
            Some(existing) if existing != price.currency => {
                return Err(ProviderCostEstimateError::MixedCurrencies);
            }
            None => currency = Some(&price.currency),
            Some(_) => {}
        }
        match identity {
            Some(existing)
                if existing
                    != (
                        price.supplier.as_str(),
                        price.provider.as_str(),
                        price.model.as_str(),
                    ) =>
            {
                return Err(ProviderCostEstimateError::MixedPriceIdentity);
            }
            None => {
                identity = Some((
                    price.supplier.as_str(),
                    price.provider.as_str(),
                    price.model.as_str(),
                ));
            }
            Some(_) => {}
        }
        if components
            .iter()
            .any(|existing: &ProviderCostEstimateComponent| {
                existing.dimension == component.dimension
            })
        {
            return Err(ProviderCostEstimateError::DuplicateDimension(
                component.dimension,
            ));
        }
        amount_units = amount_units
            .checked_add(component.amount_units)
            .ok_or(ProviderCostEstimateError::ArithmeticOverflow)?;
        components.push(component);
    }

    Ok(Some(ProviderCostRequestEstimate {
        currency: currency.unwrap_or_default().to_string(),
        amount_units,
        components,
    }))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Data type: provider cost snapshot.
pub struct ProviderCostSnapshot {
    /// Field: certainty.
    pub certainty: CostCertainty,
    /// Field: amount units.
    pub amount_units: Option<u64>,
    /// Field: currency.
    pub currency: Option<String>,
    /// Field: price version.
    pub price_version: Option<String>,
    /// Field: source reference.
    pub source_reference: Option<String>,
}

impl ProviderCostSnapshot {
    /// Constructor / associated function: estimated.
    pub fn estimated(price: &ProviderCostPrice, amount_units: u64) -> Self {
        Self {
            certainty: CostCertainty::Estimated,
            amount_units: Some(amount_units),
            currency: Some(price.currency.clone()),
            price_version: Some(price.version.clone()),
            source_reference: Some(price.source_reference.clone()),
        }
    }

    /// Constructor / associated function: known.
    pub fn known(currency: String, amount_units: u64, source_reference: String) -> Self {
        Self {
            certainty: CostCertainty::Known,
            amount_units: Some(amount_units),
            currency: Some(currency),
            price_version: None,
            source_reference: Some(source_reference),
        }
    }

    /// Constructor / associated function: unknown.
    pub fn unknown() -> Self {
        Self {
            certainty: CostCertainty::Unknown,
            amount_units: None,
            currency: None,
            price_version: None,
            source_reference: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
/// Enumeration: provider cost price error.
pub enum ProviderCostPriceError {
    #[error("provider cost price has an empty required field: {0}")]
    /// Variant: empty field.
    EmptyField(&'static str),
    #[error("provider cost price has an invalid effective window")]
    /// Variant: invalid effective window.
    InvalidEffectiveWindow,
    #[error("provider cost price versions overlap")]
    /// Variant: overlapping versions.
    OverlappingVersions,
    #[error("no effective provider cost price exists")]
    /// Variant: no effective price.
    NoEffectivePrice,
}

fn validate_price(price: &ProviderCostPrice) -> Result<(), ProviderCostPriceError> {
    for (name, value) in [
        ("supplier", price.supplier.as_str()),
        ("provider", price.provider.as_str()),
        ("model", price.model.as_str()),
        ("currency", price.currency.as_str()),
        ("version", price.version.as_str()),
        ("source_reference", price.source_reference.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(ProviderCostPriceError::EmptyField(name));
        }
    }
    if price
        .effective_to_unix_secs
        .is_some_and(|end| end <= price.effective_from_unix_secs)
    {
        return Err(ProviderCostPriceError::InvalidEffectiveWindow);
    }
    Ok(())
}

fn same_price_key(left: &ProviderCostPrice, right: &ProviderCostPrice) -> bool {
    left.supplier == right.supplier
        && left.provider == right.provider
        && left.model == right.model
        && left.dimension == right.dimension
        && left.currency == right.currency
        && left.unit == right.unit
}

fn contains(price: &ProviderCostPrice, at_unix_secs: u64) -> bool {
    price.effective_from_unix_secs <= at_unix_secs
        && price
            .effective_to_unix_secs
            .is_none_or(|end| at_unix_secs < end)
}

fn overlaps(left: &ProviderCostPrice, right: &ProviderCostPrice) -> bool {
    let left_ends_after_right_starts = left
        .effective_to_unix_secs
        .is_none_or(|end| right.effective_from_unix_secs < end);
    let right_ends_after_left_starts = right
        .effective_to_unix_secs
        .is_none_or(|end| left.effective_from_unix_secs < end);
    left_ends_after_right_starts && right_ends_after_left_starts
}

#[allow(clippy::too_many_arguments)]
/// Function: resolve provider cost price.
pub fn resolve_provider_cost_price<'a>(
    prices: &'a [ProviderCostPrice],
    supplier: &str,
    provider: &str,
    model: &str,
    dimension: ProviderCostDimension,
    currency: &str,
    unit: ProviderCostUnit,
    at_unix_secs: u64,
) -> Result<&'a ProviderCostPrice, ProviderCostPriceError> {
    for price in prices {
        validate_price(price)?;
    }

    let matching: Vec<_> = prices
        .iter()
        .filter(|price| {
            price.supplier == supplier
                && price.provider == provider
                && price.model == model
                && price.dimension == dimension
                && price.currency == currency
                && price.unit == unit
        })
        .collect();

    for (index, left) in matching.iter().enumerate() {
        if matching[index + 1..]
            .iter()
            .any(|right| same_price_key(left, right) && overlaps(left, right))
        {
            return Err(ProviderCostPriceError::OverlappingVersions);
        }
    }

    matching
        .into_iter()
        .find(|price| contains(price, at_unix_secs))
        .ok_or(ProviderCostPriceError::NoEffectivePrice)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn price(version: &str, from: u64, to: Option<u64>, amount: u64) -> ProviderCostPrice {
        price_for(
            ProviderCostDimension::Input,
            ProviderCostUnit::PerMillionTokens,
            version,
            from,
            to,
            amount,
        )
    }

    fn price_for(
        dimension: ProviderCostDimension,
        unit: ProviderCostUnit,
        version: &str,
        from: u64,
        to: Option<u64>,
        amount: u64,
    ) -> ProviderCostPrice {
        ProviderCostPrice {
            import_id: format!("import-{version}"),
            supplier: "supplier-a".into(),
            provider: "provider-a".into(),
            model: "model-a".into(),
            dimension,
            currency: "USD".into(),
            unit,
            version: version.into(),
            price_units: amount,
            effective_from_unix_secs: from,
            effective_to_unix_secs: to,
            source_reference: format!("price-book-{version}"),
            imported_by: "admin-1".into(),
        }
    }

    fn standardized_usage(
        input_tokens: i64,
        output_tokens: i64,
        cache_creation_tokens: i64,
        cache_read_tokens: i64,
    ) -> crate::StandardizedUsage {
        crate::StandardizedUsage {
            input_tokens,
            output_tokens,
            cache_creation_tokens,
            cache_read_tokens,
            ..crate::StandardizedUsage::default()
        }
    }

    #[test]
    fn maps_openai_usage_to_mutually_exclusive_token_quantities() {
        let quantities = provider_cost_token_quantities_from_standardized_usage(
            Some("openai:chat"),
            &standardized_usage(100, 30, 10, 20),
            ProviderCostInputPriceMode::ExclusiveOfCache,
        )
        .unwrap();

        assert_eq!(quantities.input, 70);
        assert_eq!(quantities.output, 30);
        assert_eq!(quantities.cache_write, 10);
        assert_eq!(quantities.cache_read, 20);
        assert_eq!(
            quantities.input + quantities.cache_write + quantities.cache_read,
            100
        );
    }

    #[test]
    fn keeps_claude_fresh_input_separate_from_cache_quantities() {
        let quantities = provider_cost_token_quantities_from_standardized_usage(
            Some("claude:messages"),
            &standardized_usage(60, 0, 15, 5),
            ProviderCostInputPriceMode::ExclusiveOfCache,
        )
        .unwrap();

        assert_eq!(quantities.input, 60);
        assert_eq!(quantities.cache_write, 15);
        assert_eq!(quantities.cache_read, 5);
    }

    #[test]
    fn leaves_cache_usage_unknown_without_explicit_semantics() {
        let usage = standardized_usage(100, 0, 10, 20);
        assert_eq!(
            provider_cost_token_quantities_from_standardized_usage(
                None,
                &usage,
                ProviderCostInputPriceMode::ExclusiveOfCache,
            ),
            None
        );
        assert_eq!(
            provider_cost_token_quantities_from_standardized_usage(
                Some("openai:chat"),
                &usage,
                ProviderCostInputPriceMode::InclusiveOfCache,
            ),
            None
        );
    }

    #[test]
    fn rejects_negative_usage_and_conflicting_cache_breakdowns() {
        assert_eq!(
            provider_cost_token_quantities_from_standardized_usage(
                Some("openai:chat"),
                &standardized_usage(-1, 0, 0, 0),
                ProviderCostInputPriceMode::ExclusiveOfCache,
            ),
            None
        );

        let usage = crate::StandardizedUsage {
            input_tokens: 100,
            cache_creation_tokens: 10,
            cache_creation_ephemeral_5m_tokens: 6,
            cache_creation_ephemeral_1h_tokens: 5,
            ..crate::StandardizedUsage::default()
        };
        assert_eq!(
            provider_cost_token_quantities_from_standardized_usage(
                Some("openai:chat"),
                &usage,
                ProviderCostInputPriceMode::ExclusiveOfCache,
            ),
            None
        );
    }

    #[test]
    fn derives_cache_write_from_classified_breakdown_when_aggregate_is_absent() {
        let usage = crate::StandardizedUsage {
            input_tokens: 100,
            cache_creation_ephemeral_5m_tokens: 6,
            cache_creation_ephemeral_1h_tokens: 4,
            ..crate::StandardizedUsage::default()
        };
        let quantities = provider_cost_token_quantities_from_standardized_usage(
            Some("openai:chat"),
            &usage,
            ProviderCostInputPriceMode::ExclusiveOfCache,
        )
        .unwrap();

        assert_eq!(quantities.input, 90);
        assert_eq!(quantities.cache_write, 10);
    }

    #[test]
    fn mapped_quantities_are_estimated_once_per_token_dimension() {
        let quantities = provider_cost_token_quantities_from_standardized_usage(
            Some("openai:chat"),
            &standardized_usage(100, 0, 10, 20),
            ProviderCostInputPriceMode::ExclusiveOfCache,
        )
        .unwrap();
        let estimate = estimate_provider_request_cost(&[
            ProviderCostEstimateInput {
                price: Some(price_for(
                    ProviderCostDimension::Input,
                    ProviderCostUnit::PerMillionTokens,
                    "input-v1",
                    0,
                    None,
                    1_000_000,
                )),
                quantity: Some(quantities.input),
            },
            ProviderCostEstimateInput {
                price: Some(price_for(
                    ProviderCostDimension::CacheWrite,
                    ProviderCostUnit::PerMillionTokens,
                    "cache-write-v1",
                    0,
                    None,
                    1_000_000,
                )),
                quantity: Some(quantities.cache_write),
            },
            ProviderCostEstimateInput {
                price: Some(price_for(
                    ProviderCostDimension::CacheRead,
                    ProviderCostUnit::PerMillionTokens,
                    "cache-read-v1",
                    0,
                    None,
                    1_000_000,
                )),
                quantity: Some(quantities.cache_read),
            },
        ])
        .unwrap()
        .unwrap();

        assert_eq!(estimate.amount_units, 100);
        assert_eq!(
            estimate
                .components
                .iter()
                .map(|component| component.quantity)
                .sum::<u64>(),
            100
        );
    }

    #[test]
    fn selects_version_with_half_open_effective_window() {
        let prices = [price("v1", 100, Some(200), 10), price("v2", 200, None, 20)];
        let selected = resolve_provider_cost_price(
            &prices,
            "supplier-a",
            "provider-a",
            "model-a",
            ProviderCostDimension::Input,
            "USD",
            ProviderCostUnit::PerMillionTokens,
            200,
        )
        .unwrap();
        assert_eq!(selected.version, "v2");
    }

    #[test]
    fn rejects_overlapping_versions() {
        let prices = [price("v1", 100, Some(201), 10), price("v2", 200, None, 20)];
        let result = resolve_provider_cost_price(
            &prices,
            "supplier-a",
            "provider-a",
            "model-a",
            ProviderCostDimension::Input,
            "USD",
            ProviderCostUnit::PerMillionTokens,
            200,
        );
        assert_eq!(result, Err(ProviderCostPriceError::OverlappingVersions));
    }

    #[test]
    fn missing_or_expired_price_is_explicit() {
        let prices = [price("v1", 100, Some(200), 10)];
        let result = resolve_provider_cost_price(
            &prices,
            "supplier-a",
            "provider-a",
            "model-a",
            ProviderCostDimension::Input,
            "USD",
            ProviderCostUnit::PerMillionTokens,
            200,
        );
        assert_eq!(result, Err(ProviderCostPriceError::NoEffectivePrice));
    }

    #[test]
    fn estimated_snapshot_freezes_selected_version_and_amount() {
        let mut selected = price("v1", 100, None, 10);
        let snapshot = ProviderCostSnapshot::estimated(&selected, 5);
        selected.version = "v2".into();
        selected.price_units = 20;
        assert_eq!(snapshot.price_version.as_deref(), Some("v1"));
        assert_eq!(snapshot.amount_units, Some(5));
        assert_eq!(snapshot.certainty, CostCertainty::Estimated);
    }

    #[test]
    fn unknown_cost_does_not_encode_zero() {
        let snapshot = ProviderCostSnapshot::unknown();
        assert_eq!(snapshot.certainty, CostCertainty::Unknown);
        assert_eq!(snapshot.amount_units, None);
        assert_eq!(snapshot.currency, None);
    }

    #[test]
    fn per_request_and_per_million_calculations_round_up_in_fixed_point_units() {
        assert_eq!(
            calculate_provider_cost_amount(
                1,
                1,
                ProviderCostUnit::PerMillionTokens,
                ProviderCostDimension::Input,
            )
            .unwrap(),
            1
        );
        assert_eq!(
            calculate_provider_cost_amount(
                3,
                7,
                ProviderCostUnit::PerRequest,
                ProviderCostDimension::Request,
            )
            .unwrap(),
            21
        );
        assert_eq!(
            calculate_provider_cost_amount(
                0,
                u64::MAX,
                ProviderCostUnit::PerRequest,
                ProviderCostDimension::Request,
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn request_estimate_sums_input_output_and_cache_components_once() {
        let input = price_for(
            ProviderCostDimension::Input,
            ProviderCostUnit::PerMillionTokens,
            "input-v1",
            0,
            None,
            2_000_000,
        );
        let output = price_for(
            ProviderCostDimension::Output,
            ProviderCostUnit::PerMillionTokens,
            "output-v1",
            0,
            None,
            3_000_000,
        );
        let cache = price_for(
            ProviderCostDimension::CacheRead,
            ProviderCostUnit::PerMillionTokens,
            "cache-v1",
            0,
            None,
            1_000_000,
        );
        let estimate = estimate_provider_request_cost(&[
            ProviderCostEstimateInput {
                price: Some(input),
                quantity: Some(500_000),
            },
            ProviderCostEstimateInput {
                price: Some(output),
                quantity: Some(1_000_000),
            },
            ProviderCostEstimateInput {
                price: Some(cache),
                quantity: Some(1),
            },
        ])
        .unwrap()
        .unwrap();

        assert_eq!(estimate.amount_units, 4_000_001);
        assert_eq!(estimate.components.len(), 3);
        assert_eq!(estimate.components[0].amount_units, 1_000_000);
        assert_eq!(estimate.components[1].amount_units, 3_000_000);
        assert_eq!(estimate.components[2].amount_units, 1);
    }

    #[test]
    fn request_estimate_freezes_each_component_price_provenance() {
        let mut input = price_for(
            ProviderCostDimension::Input,
            ProviderCostUnit::PerMillionTokens,
            "input-v1",
            0,
            None,
            1_000_000,
        );
        let mut output = price_for(
            ProviderCostDimension::Output,
            ProviderCostUnit::PerMillionTokens,
            "output-v2",
            0,
            None,
            2_000_000,
        );
        let estimate = estimate_provider_request_cost(&[
            ProviderCostEstimateInput {
                price: Some(input.clone()),
                quantity: Some(1_000_000),
            },
            ProviderCostEstimateInput {
                price: Some(output.clone()),
                quantity: Some(1_000_000),
            },
        ])
        .unwrap()
        .unwrap();
        input.version = "input-v3".into();
        output.source_reference = "revised-price-book".into();

        assert_eq!(estimate.amount_units, 3_000_000);
        assert_eq!(estimate.components[0].price_version, "input-v1");
        assert_eq!(estimate.components[1].price_version, "output-v2");
        assert_eq!(
            estimate.components[1].price_source_reference,
            "price-book-output-v2"
        );
    }

    #[test]
    fn missing_component_price_or_quantity_leaves_provider_cost_unknown() {
        assert_eq!(
            estimate_provider_request_cost(&[ProviderCostEstimateInput {
                price: Some(price("v1", 0, None, 1_000_000)),
                quantity: None,
            }])
            .unwrap(),
            None
        );
        assert_eq!(
            estimate_provider_request_cost(&[ProviderCostEstimateInput {
                price: None,
                quantity: Some(1_000_000),
            }])
            .unwrap(),
            None
        );
    }

    #[test]
    fn request_estimate_rejects_wrong_identity_and_duplicate_dimension() {
        let input = price_for(
            ProviderCostDimension::Input,
            ProviderCostUnit::PerMillionTokens,
            "input-v1",
            0,
            None,
            1_000_000,
        );
        let mut wrong_identity = price_for(
            ProviderCostDimension::Output,
            ProviderCostUnit::PerMillionTokens,
            "output-v1",
            0,
            None,
            1_000_000,
        );
        wrong_identity.provider = "provider-b".into();
        assert_eq!(
            estimate_provider_request_cost(&[
                ProviderCostEstimateInput {
                    price: Some(input.clone()),
                    quantity: Some(1),
                },
                ProviderCostEstimateInput {
                    price: Some(wrong_identity),
                    quantity: Some(1),
                },
            ]),
            Err(ProviderCostEstimateError::MixedPriceIdentity)
        );

        let mut duplicate = input.clone();
        duplicate.import_id = "import-input-v2".into();
        duplicate.version = "input-v2".into();
        assert_eq!(
            estimate_provider_request_cost(&[
                ProviderCostEstimateInput {
                    price: Some(input),
                    quantity: Some(1),
                },
                ProviderCostEstimateInput {
                    price: Some(duplicate),
                    quantity: Some(1),
                },
            ]),
            Err(ProviderCostEstimateError::DuplicateDimension(
                ProviderCostDimension::Input
            ))
        );
    }

    #[test]
    fn mismatched_units_and_overflow_do_not_estimate() {
        assert!(matches!(
            calculate_provider_cost_amount(
                1,
                1,
                ProviderCostUnit::PerRequest,
                ProviderCostDimension::CacheRead,
            ),
            Err(ProviderCostEstimateError::UnitDimensionMismatch { .. })
        ));
        assert_eq!(
            calculate_provider_cost_amount(
                u64::MAX,
                u64::MAX,
                ProviderCostUnit::PerRequest,
                ProviderCostDimension::Request,
            ),
            Err(ProviderCostEstimateError::ArithmeticOverflow)
        );
    }

    #[test]
    fn frozen_attempt_cost_survives_price_changes() {
        // A reservation admitted while v1 was effective keeps the v1 cost basis
        // even after v2 takes effect, mirroring frozen-quote settlement.
        let prices = [
            price("v1", 0, Some(200), 1_000_000),
            price("v2", 200, None, 5_000_000),
            price_for(
                ProviderCostDimension::Output,
                ProviderCostUnit::PerMillionTokens,
                "out-v1",
                0,
                None,
                1_000_000,
            ),
        ];
        let quantities = ProviderCostTokenQuantities {
            input: 1_000_000,
            output: 0,
            cache_write: 0,
            cache_read: 0,
        };
        let frozen = estimate_attempt_provider_cost(
            &prices,
            "supplier-a",
            "provider-a",
            "model-a",
            Some(&quantities),
            0,
            "USD",
            100,
        )
        .unwrap()
        .unwrap();
        assert_eq!(frozen.amount_units, 1_000_000);
        assert_eq!(frozen.components[0].price_version, "v1");

        let settled_later_at_admission = estimate_attempt_provider_cost(
            &prices,
            "supplier-a",
            "provider-a",
            "model-a",
            Some(&quantities),
            0,
            "USD",
            100,
        )
        .unwrap()
        .unwrap();
        assert_eq!(settled_later_at_admission.amount_units, 1_000_000);
    }

    #[test]
    fn attempt_cost_joins_token_cache_and_per_image_dimensions() {
        let prices = [
            price_for(
                ProviderCostDimension::Input,
                ProviderCostUnit::PerMillionTokens,
                "input-v1",
                0,
                None,
                1_000_000,
            ),
            price_for(
                ProviderCostDimension::Output,
                ProviderCostUnit::PerMillionTokens,
                "output-v1",
                0,
                None,
                2_000_000,
            ),
            price_for(
                ProviderCostDimension::CacheRead,
                ProviderCostUnit::PerMillionTokens,
                "cache-read-v1",
                0,
                None,
                500_000,
            ),
            price_for(
                ProviderCostDimension::Image,
                ProviderCostUnit::PerImage,
                "image-v1",
                0,
                None,
                30,
            ),
        ];
        let quantities = ProviderCostTokenQuantities {
            input: 500_000,
            output: 1_000_000,
            cache_write: 0,
            cache_read: 2_000_000,
        };
        let estimate = estimate_attempt_provider_cost(
            &prices,
            "supplier-a",
            "provider-a",
            "model-a",
            Some(&quantities),
            2,
            "USD",
            50,
        )
        .unwrap()
        .unwrap();
        assert_eq!(estimate.amount_units, 500_000 + 2_000_000 + 1_000_000 + 60);
        assert_eq!(estimate.components.len(), 4);
    }

    #[test]
    fn missing_frozen_price_leaves_attempt_cost_unknown() {
        let prices = [price("v2", 200, None, 5_000_000)];
        let quantities = ProviderCostTokenQuantities {
            input: 1,
            output: 0,
            cache_write: 0,
            cache_read: 0,
        };
        assert_eq!(
            estimate_attempt_provider_cost(
                &prices,
                "supplier-a",
                "provider-a",
                "model-a",
                Some(&quantities),
                0,
                "USD",
                100,
            )
            .unwrap(),
            None
        );
        assert_eq!(
            estimate_attempt_provider_cost(
                &prices,
                "supplier-a",
                "provider-a",
                "model-a",
                None,
                0,
                "USD",
                100,
            )
            .unwrap(),
            None
        );
    }
}
