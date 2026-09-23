use aether_crypto::{encrypt_python_fernet_plaintext, looks_like_python_fernet_ciphertext};
use aether_data_contracts::repository::provider_catalog::ProviderCatalogKeyCredentialsCasUpdate;
use tracing::{info, warn};

use crate::handlers::shared::{
    decrypt_catalog_secret_with_fallbacks, effective_catalog_encryption_key,
};
use crate::{AppState, GatewayError};

use super::system_config_bool;

const SWEEP_BATCH_SIZE: i32 = 200;
const MAX_SWEEP_BATCHES: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub(crate) struct ProviderCredentialSweepSummary {
    pub(crate) scanned: usize,
    pub(crate) migrated: usize,
    pub(crate) reencrypted: usize,
    pub(crate) skipped_undecryptable: usize,
    pub(crate) conflicts: usize,
    pub(crate) failed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LegacyCredentialAction {
    /// The legacy column already holds ciphertext that decrypts with the
    /// effective key; move it to `encrypted_key` unchanged.
    MoveCiphertext,
    /// The legacy column holds a plaintext secret; encrypt it before moving.
    Reencrypt(String),
    /// The value looks like fernet ciphertext but does not decrypt with any
    /// configured key. Leave the row untouched for manual repair.
    SkipUndecryptable,
}

fn classify_legacy_credential(
    encryption_key: Option<&str>,
    stored: &str,
) -> LegacyCredentialAction {
    if decrypt_catalog_secret_with_fallbacks(encryption_key, stored).is_some() {
        return LegacyCredentialAction::MoveCiphertext;
    }
    if looks_like_python_fernet_ciphertext(stored) {
        return LegacyCredentialAction::SkipUndecryptable;
    }
    LegacyCredentialAction::Reencrypt(stored.to_string())
}

/// One-time/idempotent sweep that moves provider key credentials out of the
/// legacy plaintext-first `api_key` column into `encrypted_key` (see ADR
/// `docs/adr/provider-api-key-plaintext-cleanup.md`). Rows already migrated
/// are never selected again, so reruns are cheap no-ops.
pub(crate) async fn perform_provider_credential_sweep_once(
    state: &AppState,
) -> Result<ProviderCredentialSweepSummary, GatewayError> {
    let mut summary = ProviderCredentialSweepSummary::default();
    if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
        return Ok(summary);
    }
    if !system_config_bool(&state.data, "enable_provider_credential_sweep", true)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?
    {
        return Ok(summary);
    }
    let encryption_key = effective_catalog_encryption_key(state);
    if encryption_key.is_none() {
        warn!(
            event_name = "provider_credential_sweep_skipped",
            log_type = "ops",
            worker = "provider_credential_sweep",
            "provider credential sweep skipped: no catalog encryption key configured"
        );
        return Ok(summary);
    }
    let encryption_key = encryption_key.map(|key| key.into_owned());

    for _ in 0..MAX_SWEEP_BATCHES {
        let keys = state
            .data
            .list_provider_catalog_keys_with_legacy_credential(SWEEP_BATCH_SIZE)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if keys.is_empty() {
            break;
        }
        let mut progressed = false;
        for key in keys {
            summary.scanned += 1;
            let Some(stored) = key.encrypted_api_key.clone() else {
                continue;
            };
            let (new_ciphertext, reencrypted) = match classify_legacy_credential(
                encryption_key.as_deref(),
                stored.trim(),
            ) {
                LegacyCredentialAction::MoveCiphertext => (stored.clone(), false),
                LegacyCredentialAction::Reencrypt(plaintext) => {
                    match encrypt_python_fernet_plaintext(
                        encryption_key.as_deref().unwrap_or_default(),
                        &plaintext,
                    ) {
                        Ok(ciphertext) => (ciphertext, true),
                        Err(err) => {
                            summary.failed += 1;
                            warn!(
                                event_name = "provider_credential_sweep_failed",
                                log_type = "ops",
                                worker = "provider_credential_sweep",
                                key_id = %key.id,
                                error = %err,
                                "failed to encrypt legacy provider credential during sweep"
                            );
                            continue;
                        }
                    }
                }
                LegacyCredentialAction::SkipUndecryptable => {
                    summary.skipped_undecryptable += 1;
                    warn!(
                        event_name = "provider_credential_sweep_skipped",
                        log_type = "ops",
                        worker = "provider_credential_sweep",
                        key_id = %key.id,
                        "legacy provider credential is ciphertext but decrypts with no configured key; leaving row untouched"
                    );
                    continue;
                }
            };
            let update = ProviderCatalogKeyCredentialsCasUpdate {
                key_id: key.id.clone(),
                expected_provider_id: key.provider_id.clone(),
                expected_encrypted_api_key: Some(stored.clone()),
                expected_encrypted_auth_config: key.encrypted_auth_config.clone(),
                encrypted_api_key: Some(new_ciphertext),
                encrypted_auth_config: key.encrypted_auth_config.clone(),
            };
            match state
                .data
                .compare_and_swap_provider_catalog_key_credentials(&update)
                .await
            {
                Ok(true) => {
                    summary.migrated += 1;
                    if reencrypted {
                        summary.reencrypted += 1;
                    }
                    progressed = true;
                }
                Ok(false) => {
                    // The row changed underneath the sweep; leave it for the
                    // next run, which will observe the winning record.
                    summary.conflicts += 1;
                }
                Err(err) => {
                    summary.failed += 1;
                    warn!(
                        event_name = "provider_credential_sweep_failed",
                        log_type = "ops",
                        worker = "provider_credential_sweep",
                        key_id = %key.id,
                        error = %err,
                        "provider credential sweep CAS failed"
                    );
                }
            }
        }
        if !progressed {
            break;
        }
    }
    if summary.migrated > 0 {
        info!(
            event_name = "provider_credential_sweep_completed",
            log_type = "ops",
            worker = "provider_credential_sweep",
            scanned = summary.scanned,
            migrated = summary.migrated,
            reencrypted = summary.reencrypted,
            "provider credential sweep migrated legacy-column credentials into encrypted_key"
        );
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;

    #[test]
    fn classify_moves_existing_ciphertext_without_reencrypting() {
        let secret = "sk-live-secret";
        let ciphertext =
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, secret).expect("encrypt");
        // In test builds the dev key is an accepted fallback for decryption.
        match classify_legacy_credential(Some("other-key"), &ciphertext) {
            LegacyCredentialAction::MoveCiphertext => {}
            other => panic!("expected MoveCiphertext, got {other:?}"),
        }
    }

    #[test]
    fn classify_reencrypts_plaintext_legacy_value() {
        match classify_legacy_credential(Some(DEVELOPMENT_ENCRYPTION_KEY), "plain-secret") {
            LegacyCredentialAction::Reencrypt(value) => assert_eq!(value, "plain-secret"),
            other => panic!("expected Reencrypt, got {other:?}"),
        }
    }

    #[test]
    fn classify_skips_ciphertext_that_decrypts_with_no_key() {
        let foreign = encrypt_python_fernet_plaintext("sweep-unconfigured-key", "secret")
            .expect("encrypt with a key the sweep does not know");
        match classify_legacy_credential(Some(DEVELOPMENT_ENCRYPTION_KEY), &foreign) {
            LegacyCredentialAction::SkipUndecryptable => {}
            other => panic!("expected SkipUndecryptable, got {other:?}"),
        }
    }
}
