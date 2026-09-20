use std::sync::atomic::{AtomicUsize, Ordering};

use aether_data_contracts::repository::settlement::{StoredUsageSettlement, UsageSettlementInput};
use aether_data_contracts::repository::usage::StoredRequestUsageAudit;
use aether_data_contracts::{DataLayerError, DataLayerError::InvalidInput};
use async_trait::async_trait;

use super::{sample_usage, settle_usage_if_needed, UsageSettlementWriter};

struct ProviderCostCaptureWriter {
    has_writer: bool,
    settlement: Option<StoredUsageSettlement>,
    settlement_calls: AtomicUsize,
    capture_calls: AtomicUsize,
    capture_fails: bool,
}

impl ProviderCostCaptureWriter {
    fn with_settlement(billing_status: &str) -> Self {
        Self {
            has_writer: true,
            settlement: Some(StoredUsageSettlement {
                request_id: "req-1".to_string(),
                wallet_id: Some("wallet-1".to_string()),
                billing_status: billing_status.to_string(),
                wallet_balance_before: Some(1.0),
                wallet_balance_after: Some(0.25),
                wallet_recharge_balance_before: None,
                wallet_recharge_balance_after: None,
                wallet_gift_balance_before: None,
                wallet_gift_balance_after: None,
                provider_monthly_used_usd: None,
                finalized_at_unix_secs: Some(200),
            }),
            settlement_calls: AtomicUsize::new(0),
            capture_calls: AtomicUsize::new(0),
            capture_fails: false,
        }
    }
}

#[async_trait]
impl UsageSettlementWriter for ProviderCostCaptureWriter {
    fn has_usage_settlement_writer(&self) -> bool {
        self.has_writer
    }

    async fn settle_usage(
        &self,
        _input: UsageSettlementInput,
    ) -> Result<Option<StoredUsageSettlement>, DataLayerError> {
        self.settlement_calls.fetch_add(1, Ordering::AcqRel);
        Ok(self.settlement.clone())
    }

    async fn capture_provider_cost_for_usage(
        &self,
        usage: &StoredRequestUsageAudit,
    ) -> Result<(), DataLayerError> {
        assert_eq!(usage.request_id, "req-1");
        self.capture_calls.fetch_add(1, Ordering::AcqRel);
        if self.capture_fails {
            return Err(InvalidInput("provider cost capture failed".to_string()));
        }
        Ok(())
    }
}

#[tokio::test]
async fn captures_provider_cost_after_a_confirmed_completed_settlement() {
    let writer = ProviderCostCaptureWriter::with_settlement("settled");

    settle_usage_if_needed(&writer, &sample_usage())
        .await
        .expect("settlement and capture should succeed");

    assert_eq!(writer.settlement_calls.load(Ordering::Acquire), 1);
    assert_eq!(writer.capture_calls.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn does_not_capture_when_pending_settlement_returns_no_row() {
    let writer = ProviderCostCaptureWriter {
        has_writer: true,
        settlement: None,
        settlement_calls: AtomicUsize::new(0),
        capture_calls: AtomicUsize::new(0),
        capture_fails: false,
    };

    settle_usage_if_needed(&writer, &sample_usage())
        .await
        .expect("empty settlement result is not an error");

    assert_eq!(writer.settlement_calls.load(Ordering::Acquire), 1);
    assert_eq!(writer.capture_calls.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn captures_completed_already_finalized_usage_during_replay() {
    let writer = ProviderCostCaptureWriter::with_settlement("settled");
    let mut usage = sample_usage();
    usage.billing_status = "settled".to_string();

    settle_usage_if_needed(&writer, &usage)
        .await
        .expect("replay capture should succeed");

    assert_eq!(writer.settlement_calls.load(Ordering::Acquire), 0);
    assert_eq!(writer.capture_calls.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn capture_failure_returns_error_after_the_settlement_call() {
    let mut writer = ProviderCostCaptureWriter::with_settlement("settled");
    writer.capture_fails = true;

    let result = settle_usage_if_needed(&writer, &sample_usage()).await;

    assert!(matches!(result, Err(DataLayerError::InvalidInput(_))));
    assert_eq!(writer.settlement_calls.load(Ordering::Acquire), 1);
    assert_eq!(writer.capture_calls.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn skips_capture_without_a_writer_or_for_ineligible_usage() {
    let mut no_writer = ProviderCostCaptureWriter::with_settlement("settled");
    no_writer.has_writer = false;
    settle_usage_if_needed(&no_writer, &sample_usage())
        .await
        .expect("missing writer should skip");
    assert_eq!(no_writer.capture_calls.load(Ordering::Acquire), 0);

    for status in ["pending", "streaming"] {
        let writer = ProviderCostCaptureWriter::with_settlement("settled");
        let mut usage = sample_usage();
        usage.status = status.to_string();
        settle_usage_if_needed(&writer, &usage)
            .await
            .expect("nonterminal usage should skip");
        assert_eq!(writer.settlement_calls.load(Ordering::Acquire), 0);
        assert_eq!(writer.capture_calls.load(Ordering::Acquire), 0);
    }

    let failed_writer = ProviderCostCaptureWriter::with_settlement("settled");
    let mut failed_usage = sample_usage();
    failed_usage.status = "failed".to_string();
    settle_usage_if_needed(&failed_writer, &failed_usage)
        .await
        .expect("failed usage may settle but must not capture");
    assert_eq!(failed_writer.settlement_calls.load(Ordering::Acquire), 1);
    assert_eq!(failed_writer.capture_calls.load(Ordering::Acquire), 0);

    let void_writer = ProviderCostCaptureWriter::with_settlement("settled");
    let mut void_usage = sample_usage();
    void_usage.billing_status = "void".to_string();
    settle_usage_if_needed(&void_writer, &void_usage)
        .await
        .expect("void usage should skip");
    assert_eq!(void_writer.settlement_calls.load(Ordering::Acquire), 0);
    assert_eq!(void_writer.capture_calls.load(Ordering::Acquire), 0);
}
