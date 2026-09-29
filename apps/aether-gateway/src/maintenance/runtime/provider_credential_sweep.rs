use aether_crypto::{encrypt_python_fernet_plaintext, looks_like_python_fernet_ciphertext};
use aether_data_contracts::repository::provider_catalog::ProviderCatalogKeyCredentialsCasUpdate;
use tracing::{info, warn};

use crate::handlers::shared::{
    decrypt_catalog_secret_with_fallbacks, effective_catalog_encryption_key,
};
use crate::important_notification::{
    important_notification_dispatch_ready_for_item, send_important_notification_for_item,
    ImportantNotification, PROVIDER_CREDENTIAL_UNDECRYPTABLE_ITEM_KEY,
};
use crate::{AppState, GatewayError};

use super::system_config_bool;

const SWEEP_BATCH_SIZE: i32 = 200;
const MAX_SWEEP_BATCHES: usize = 500;
const PROVIDER_CREDENTIAL_UNDECRYPTABLE_STATE_KEY: &str = "provider_ops:credential_undecryptable";
const PROVIDER_CREDENTIAL_UNDECRYPTABLE_REPEAT_COOLDOWN_SECS: u64 = 24 * 60 * 60;
const PROVIDER_CREDENTIAL_UNDECRYPTABLE_MAX_LISTED_KEYS: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub(crate) struct ProviderCredentialSweepSummary {
    pub(crate) scanned: usize,
    pub(crate) migrated: usize,
    pub(crate) reencrypted: usize,
    pub(crate) skipped_undecryptable: usize,
    pub(crate) conflicts: usize,
    pub(crate) failed: usize,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct ProviderCredentialUndecryptableRuntimeState {
    #[serde(default)]
    last_notified_at: Option<u64>,
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
    let mut undecryptable_key_ids = Vec::<String>::new();

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
                    undecryptable_key_ids.push(key.id.clone());
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
    if summary.skipped_undecryptable > 0 {
        maybe_notify_provider_credential_undecryptable(state, &undecryptable_key_ids).await;
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

/// Decide whether this sweep run should dispatch the undecryptable-credential
/// admin notification. Undecryptable rows stay in the legacy column and are
/// re-encountered on every sweep, so a cooldown bounds the repeats; delivery
/// is allowed again once the cooldown elapsed or nothing was delivered before.
fn provider_credential_undecryptable_should_notify(
    undecryptable_key_ids: &[String],
    last_notified_at: Option<u64>,
    now_unix_secs: u64,
) -> bool {
    if undecryptable_key_ids.is_empty() {
        return false;
    }
    last_notified_at
        .map(|last| {
            now_unix_secs.saturating_sub(last)
                >= PROVIDER_CREDENTIAL_UNDECRYPTABLE_REPEAT_COOLDOWN_SECS
        })
        .unwrap_or(true)
}

fn provider_credential_undecryptable_key_list(undecryptable_key_ids: &[String]) -> String {
    let listed = undecryptable_key_ids
        .iter()
        .take(PROVIDER_CREDENTIAL_UNDECRYPTABLE_MAX_LISTED_KEYS)
        .map(|key_id| format!("`{key_id}`"))
        .collect::<Vec<_>>()
        .join("、");
    let omitted = undecryptable_key_ids
        .len()
        .saturating_sub(PROVIDER_CREDENTIAL_UNDECRYPTABLE_MAX_LISTED_KEYS);
    if omitted > 0 {
        format!("{listed} 等 {omitted} 个未列出")
    } else {
        listed
    }
}

fn build_provider_credential_undecryptable_notification(
    undecryptable_key_ids: &[String],
) -> ImportantNotification {
    let key_list = provider_credential_undecryptable_key_list(undecryptable_key_ids);
    let title = "号池凭据无法解密".to_string();
    let markdown_body = format!(
        "以下提供商密钥的凭据无法使用当前配置的加密密钥解密，已在候选解析与凭据巡检中隔离。\
         请在管理端删除并重新添加这些密钥：\n\n{key_list}\n\n受影响密钥数：{}",
        undecryptable_key_ids.len()
    );
    let text_body = format!(
        "以下提供商密钥的凭据无法使用当前配置的加密密钥解密，已在候选解析与凭据巡检中隔离。\
         请在管理端删除并重新添加这些密钥：{key_list}（受影响密钥数：{}）",
        undecryptable_key_ids.len()
    );
    ImportantNotification {
        title,
        markdown_body,
        text_body,
    }
}

fn provider_credential_undecryptable_notification_variables(
    undecryptable_key_ids: &[String],
) -> Vec<(&'static str, String)> {
    vec![
        (
            "key_ids",
            undecryptable_key_ids
                .iter()
                .take(PROVIDER_CREDENTIAL_UNDECRYPTABLE_MAX_LISTED_KEYS)
                .cloned()
                .collect::<Vec<_>>()
                .join(","),
        ),
        ("key_count", undecryptable_key_ids.len().to_string()),
    ]
}

/// Dispatches the administrator-facing `provider_credential_undecryptable`
/// notification after a sweep isolates ciphertext it cannot decrypt with any
/// configured key. Follows the provider quota/pool alert notification family:
/// gated on the important-notification module and deduplicated through the
/// runtime KV cooldown so unrepaired rows do not re-alert on every sweep.
async fn maybe_notify_provider_credential_undecryptable(
    state: &AppState,
    undecryptable_key_ids: &[String],
) {
    if undecryptable_key_ids.is_empty() {
        return;
    }
    match important_notification_dispatch_ready_for_item(
        state,
        PROVIDER_CREDENTIAL_UNDECRYPTABLE_ITEM_KEY,
    )
    .await
    {
        Ok(true) => {}
        Ok(false) => return,
        Err(err) => {
            warn!(
                event_name = "provider_credential_sweep_notification_failed",
                log_type = "ops",
                worker = "provider_credential_sweep",
                error = ?err,
                "failed to read notification readiness for undecryptable provider credentials"
            );
            return;
        }
    }
    let now_unix_secs = now_unix_secs();
    let last_notified_at = state
        .runtime_kv_get(PROVIDER_CREDENTIAL_UNDECRYPTABLE_STATE_KEY)
        .await
        .ok()
        .flatten()
        .and_then(|raw| {
            serde_json::from_str::<ProviderCredentialUndecryptableRuntimeState>(&raw).ok()
        })
        .and_then(|runtime_state| runtime_state.last_notified_at);
    if !provider_credential_undecryptable_should_notify(
        undecryptable_key_ids,
        last_notified_at,
        now_unix_secs,
    ) {
        return;
    }

    let report = send_important_notification_for_item(
        state,
        PROVIDER_CREDENTIAL_UNDECRYPTABLE_ITEM_KEY,
        build_provider_credential_undecryptable_notification(undecryptable_key_ids),
        &provider_credential_undecryptable_notification_variables(undecryptable_key_ids),
    )
    .await;
    let delivered = match &report {
        Ok(report) if report.success => true,
        Ok(report) => {
            warn!(
                event_name = "provider_credential_sweep_notification_failed",
                log_type = "ops",
                worker = "provider_credential_sweep",
                report = ?report,
                "undecryptable provider credential notification did not reach any channel"
            );
            false
        }
        Err(err) => {
            warn!(
                event_name = "provider_credential_sweep_notification_failed",
                log_type = "ops",
                worker = "provider_credential_sweep",
                error = ?err,
                "failed to send undecryptable provider credential notification"
            );
            false
        }
    };
    if !delivered {
        return;
    }

    let Ok(serialized) = serde_json::to_string(&ProviderCredentialUndecryptableRuntimeState {
        last_notified_at: Some(now_unix_secs),
    }) else {
        return;
    };
    if let Err(err) = state
        .runtime_state()
        .kv_set(
            PROVIDER_CREDENTIAL_UNDECRYPTABLE_STATE_KEY,
            serialized,
            None,
        )
        .await
    {
        warn!(
            error = %err,
            worker = "provider_credential_sweep",
            "failed to write provider credential undecryptable notification state"
        );
    }
}

fn now_unix_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
    use base64::Engine;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

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

    #[test]
    fn undecryptable_notification_does_not_send_without_skipped_keys() {
        // No undecryptable rows in this run: nothing may be dispatched and the
        // cooldown state must stay untouched so a later failure still alerts.
        assert!(!provider_credential_undecryptable_should_notify(
            &[],
            None,
            1_000
        ));
    }

    #[test]
    fn undecryptable_notification_repeats_only_after_cooldown() {
        let key_ids = ["key-1".to_string()];
        let now = PROVIDER_CREDENTIAL_UNDECRYPTABLE_REPEAT_COOLDOWN_SECS + 1_000;
        assert!(provider_credential_undecryptable_should_notify(
            &key_ids, None, now
        ));
        assert!(!provider_credential_undecryptable_should_notify(
            &key_ids,
            Some(now - PROVIDER_CREDENTIAL_UNDECRYPTABLE_REPEAT_COOLDOWN_SECS + 60),
            now,
        ));
        assert!(provider_credential_undecryptable_should_notify(
            &key_ids,
            Some(now - PROVIDER_CREDENTIAL_UNDECRYPTABLE_REPEAT_COOLDOWN_SECS),
            now,
        ));
    }

    #[test]
    fn undecryptable_notification_lists_key_ids_and_readd_guidance() {
        let notification = build_provider_credential_undecryptable_notification(&[
            "key-1".to_string(),
            "key-2".to_string(),
        ]);
        assert_eq!(notification.title, "号池凭据无法解密");
        for body in [&notification.markdown_body, &notification.text_body] {
            assert!(body.contains("`key-1`"), "body: {body}");
            assert!(body.contains("`key-2`"), "body: {body}");
            assert!(body.contains("重新添加"), "body: {body}");
            assert!(body.contains("受影响密钥数：2"), "body: {body}");
        }

        let variables = provider_credential_undecryptable_notification_variables(&[
            "key-1".to_string(),
            "key-2".to_string(),
        ]);
        assert_eq!(
            variables,
            vec![
                ("key_ids", "key-1,key-2".to_string()),
                ("key_count", "2".to_string()),
            ]
        );
    }

    #[test]
    fn undecryptable_notification_bounds_listed_key_ids() {
        let key_ids = (0..(PROVIDER_CREDENTIAL_UNDECRYPTABLE_MAX_LISTED_KEYS + 5))
            .map(|index| format!("key-{index}"))
            .collect::<Vec<_>>();
        let notification = build_provider_credential_undecryptable_notification(&key_ids);
        assert!(notification.markdown_body.contains("等 5 个未列出"));
        assert!(!notification.markdown_body.contains("`key-24`"));
    }

    /// Stand-in for the SMTP edge, mirroring the synthetic server used by the
    /// recharge recovery notification tests.
    async fn synthetic_smtp_server(
        connections: usize,
    ) -> (u16, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let mut bodies = Vec::new();
            for _ in 0..connections {
                let (socket, _) = listener.accept().await.unwrap();
                let (input, mut output) = socket.into_split();
                let mut reader = BufReader::new(input);
                output
                    .write_all(b"220 local synthetic SMTP\r\n")
                    .await
                    .unwrap();
                let mut in_body = false;
                let mut body = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).await.unwrap() == 0 {
                        break;
                    }
                    if in_body {
                        if line == ".\r\n" {
                            output.write_all(b"250 accepted\r\n").await.unwrap();
                            bodies.push(body.clone());
                            in_body = false;
                        } else {
                            body.push_str(&line);
                        }
                    } else if line.starts_with("EHLO")
                        || line.starts_with("MAIL FROM")
                        || line.starts_with("RCPT TO")
                    {
                        output.write_all(b"250 ok\r\n").await.unwrap();
                    } else if line.starts_with("DATA") {
                        in_body = true;
                        output.write_all(b"354 continue\r\n").await.unwrap();
                    } else if line.starts_with("QUIT") {
                        output.write_all(b"221 bye\r\n").await.unwrap();
                        break;
                    } else {
                        panic!("unexpected synthetic SMTP command");
                    }
                }
            }
            bodies
        });
        (port, server)
    }

    fn decoded_mime_text_body(message: &str) -> String {
        let (headers, body) = message.split_once("\r\n\r\n").expect("MIME headers");
        let content_type = headers
            .lines()
            .find(|line| line.starts_with("Content-Type: multipart/alternative;"))
            .expect("multipart content type");
        let boundary = content_type
            .split("boundary=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let delimiter = format!("--{boundary}");
        for part in body.split(&delimiter).skip(1) {
            if part.starts_with("--") {
                break;
            }
            let (part_headers, encoded) = part
                .trim_start_matches("\r\n")
                .split_once("\r\n\r\n")
                .expect("part headers");
            if part_headers
                .lines()
                .any(|line| line.starts_with("Content-Type: text/plain"))
            {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(encoded.split_whitespace().collect::<String>())
                    .expect("valid base64 MIME part");
                return String::from_utf8(bytes).expect("UTF-8 MIME part");
            }
        }
        panic!("message should carry a text/plain part");
    }

    #[tokio::test]
    async fn sweep_notifies_admin_for_undecryptable_keys_and_respects_cooldown() {
        // The synthetic SMTP edge serves exactly the deliveries the test
        // expects: the initial alert plus one re-alert after the cooldown.
        let (port, server) = synthetic_smtp_server(2).await;
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::disabled().with_system_config_values_for_tests(
                    vec![
                    (
                        crate::important_notification::IMPORTANT_NOTIFICATION_ENABLED_KEY.into(),
                        serde_json::json!(true),
                    ),
                    (
                        crate::important_notification::IMPORTANT_NOTIFICATION_EMAIL_ENABLED_KEY
                            .into(),
                        serde_json::json!(true),
                    ),
                    (
                        crate::important_notification::IMPORTANT_NOTIFICATION_DEFAULT_CHANNEL_KEY
                            .into(),
                        serde_json::json!("email"),
                    ),
                    (
                        crate::important_notification::IMPORTANT_NOTIFICATION_EMAIL_RECIPIENTS_KEY
                            .into(),
                        serde_json::json!(["admin@example.invalid"]),
                    ),
                    (
                        crate::important_notification::IMPORTANT_NOTIFICATION_ITEMS_KEY.into(),
                        serde_json::json!([{
                            "key": PROVIDER_CREDENTIAL_UNDECRYPTABLE_ITEM_KEY,
                            "enabled": true
                        }]),
                    ),
                    ("smtp_host".into(), serde_json::json!("127.0.0.1")),
                    ("smtp_port".into(), serde_json::json!(port)),
                    ("smtp_use_tls".into(), serde_json::json!(false)),
                    ("smtp_use_ssl".into(), serde_json::json!(false)),
                    (
                        "smtp_from_email".into(),
                        serde_json::json!("sweep@example.invalid"),
                    ),
                ],
                ),
            );

        // No skipped keys in this run: nothing is dispatched or recorded.
        maybe_notify_provider_credential_undecryptable(&state, &[]).await;
        assert!(state
            .runtime_kv_get(PROVIDER_CREDENTIAL_UNDECRYPTABLE_STATE_KEY)
            .await
            .expect("runtime kv reads should not fail")
            .is_none());

        // First run with affected keys delivers the notification containing
        // the key ids and records the cooldown state.
        tokio::time::timeout(
            Duration::from_secs(10),
            maybe_notify_provider_credential_undecryptable(&state, &["key-1".to_string()]),
        )
        .await
        .expect("notification dispatch should not hang");
        let cooldown_state = state
            .runtime_kv_get(PROVIDER_CREDENTIAL_UNDECRYPTABLE_STATE_KEY)
            .await
            .expect("runtime kv reads should not fail")
            .expect("delivered notification should record the cooldown state");
        assert!(cooldown_state.contains("last_notified_at"));

        // Once the cooldown elapses the still-unrepaired rows re-alert; an
        // immediate rerun stays silent instead (pure cooldown test above).
        let expired = serde_json::to_string(&ProviderCredentialUndecryptableRuntimeState {
            last_notified_at: Some(
                serde_json::from_str::<ProviderCredentialUndecryptableRuntimeState>(
                    &cooldown_state,
                )
                .expect("cooldown state should parse")
                .last_notified_at
                .expect("cooldown state should carry a timestamp")
                .saturating_sub(PROVIDER_CREDENTIAL_UNDECRYPTABLE_REPEAT_COOLDOWN_SECS + 1),
            ),
        })
        .expect("cooldown state should serialize");
        state
            .runtime_state()
            .kv_set(PROVIDER_CREDENTIAL_UNDECRYPTABLE_STATE_KEY, expired, None)
            .await
            .expect("runtime kv writes should not fail");
        tokio::time::timeout(
            Duration::from_secs(10),
            maybe_notify_provider_credential_undecryptable(&state, &["key-1".to_string()]),
        )
        .await
        .expect("notification dispatch should not hang");

        let bodies = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("synthetic SMTP exchanges should finish")
            .expect("synthetic SMTP server should not panic");
        assert_eq!(bodies.len(), 2, "exactly the two expected deliveries");
        for message in &bodies {
            let text_body = decoded_mime_text_body(message);
            assert!(text_body.contains("`key-1`"), "body: {text_body}");
            assert!(text_body.contains("重新添加"), "body: {text_body}");
        }
    }
}
