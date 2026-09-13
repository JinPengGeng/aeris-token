//! An ignored, real-PostgreSQL exercise of the operator-facing restore binary.
use std::path::{Path, PathBuf};

use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use super::executor::encrypt_backup_bytes;

#[cfg(unix)]
fn write_private(path: &Path, value: &[u8]) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .expect("private fixture should be created");
    file.write_all(value)
        .expect("private fixture should be written");
}

#[cfg(not(unix))]
fn write_private(path: &Path, value: &[u8]) {
    std::fs::write(path, value).expect("private fixture should be written");
}

fn fixture_directory() -> PathBuf {
    let directory = std::fs::canonicalize(std::env::temp_dir())
        .expect("temporary root should be real")
        .join(format!("aether-restore-drill-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).expect("fixture directory should be new");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
            .expect("fixture directory should be private");
    }
    directory
}

async fn invoke_restore(
    binary: &Path,
    directory: &Path,
    output_name: &str,
) -> std::process::Output {
    let binary = binary.to_owned();
    let directory = directory.to_owned();
    let output_name = output_name.to_owned();
    tokio::task::spawn_blocking(move || {
        std::process::Command::new(binary)
            .arg("--input")
            .arg(directory.join("backup.encrypted"))
            .arg("--object-key")
            .arg("drill/aether-data-backup-20260913-000000.json.zst.aes256gcm")
            .arg("--output")
            .arg(directory.join(output_name))
            .arg("--key-file")
            .arg(directory.join("key"))
            .arg("--apply-to-empty-drill-database")
            .arg("--database-url-file")
            .arg(directory.join("database-url"))
            .arg("--data-key-file")
            .arg(directory.join("key"))
            .env_remove("AETHER_BACKUP_HISTORICAL_KEYS_JSON")
            .env_remove("AETHER_BACKUP_KEYRING_FILE")
            .env_remove("AETHER_BACKUP_ENCRYPTION_KEY")
            .env_remove("AETHER_GATEWAY_DATA_ENCRYPTION_KEY")
            .env_remove("ENCRYPTION_KEY")
            .output()
            .expect("restore binary should execute")
    })
    .await
    .expect("restore process task should finish")
}

#[tokio::test]
#[ignore = "requires an empty aether_restore_drill_* database and a built backup-restore binary"]
async fn live_authenticated_restore_cli_preserves_credentials_wallets_and_aggregates() {
    let database_url = std::env::var("AETHER_TEST_RESTORE_DATABASE_URL")
        .expect("explicit disposable restore database is required");
    let binary = PathBuf::from(
        std::env::var("AETHER_TEST_BACKUP_RESTORE_BIN")
            .expect("explicit built backup-restore binary is required"),
    );
    let pool = PgPool::connect(&database_url)
        .await
        .expect("test database should connect");
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .expect("database identity should load");
    assert!(
        database.starts_with("aether_restore_drill_"),
        "only a disposable restore database is accepted"
    );
    let tables: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pg_catalog.pg_tables WHERE schemaname = 'public'")
            .fetch_one(&pool)
            .await
            .expect("schema should be inspected");
    assert_eq!(
        tables, 0,
        "exercise requires a fresh empty database and never clears existing data"
    );
    aether_data::lifecycle::migrate::prepare_database_for_startup(&pool)
        .await
        .expect("production bootstrap should run");
    aether_data::lifecycle::migrate::run_migrations(&pool)
        .await
        .expect("production migrations should run");

    let password = "restore-drill-password-123";
    let key = "sk-restore-drill-user-key";
    let password_hash = bcrypt::hash(password, 4).expect("fixture password should hash");
    let key_hash = format!("{:x}", Sha256::digest(key.as_bytes()));
    let wallet = json!({
        "balance": 20.0, "recharge_balance": 15.0, "gift_balance": 5.0,
        "limit_mode": "finite", "currency": "USD", "status": "active",
        "total_recharged": 20.0, "total_consumed": 1.25,
        "total_refunded": 0.0, "total_adjusted": 1.25
    });
    let aggregate = json!({
        "date_unix_secs": 1789171200, "total_requests": 1,
        "success_requests": 1, "error_requests": 0,
        "input_tokens": 17, "output_tokens": 23,
        "cache_creation_tokens": 0, "cache_read_tokens": 0,
        "total_cost": 1.25, "actual_total_cost": 1.25,
        "is_complete": true, "aggregated_at_unix_secs": 1789171200
    });
    let source = json!({
        "version": "1.0", "exported_at": "2026-09-13T00:00:00Z", "merge_mode": "overwrite",
        "config_data": {"version": "2.3", "exported_at": "2026-09-13T00:00:00Z",
            "global_models": [], "providers": [], "proxy_nodes": [], "oauth_providers": [],
            "system_configs": []},
        "user_data": {"version": "1.5", "exported_at": "2026-09-13T00:00:00Z",
            "user_groups": [], "users": [{"id": "source-drill-user",
                "email": "restore-drill@example.com", "email_verified": true,
                "username": "restore-drill", "password_hash": password_hash,
                "role": "user", "is_active": true, "wallet": wallet,
                "api_keys": [{"api_key_id": "source-drill-key", "key": key,
                    "key_hash": key_hash, "name": "Drill key", "is_active": true}]}],
            "standalone_keys": [], "usage_aggregates": {
                "stats_daily": [aggregate], "stats_user_daily": [], "stats_daily_api_key": []}}
    });
    let directory = fixture_directory();
    write_private(
        &directory.join("key"),
        DEVELOPMENT_ENCRYPTION_KEY.as_bytes(),
    );
    write_private(&directory.join("database-url"), database_url.as_bytes());
    let compressed =
        zstd::stream::encode_all(serde_json::to_vec(&source).unwrap().as_slice(), 0).unwrap();
    let (encrypted, _) = encrypt_backup_bytes(
        DEVELOPMENT_ENCRYPTION_KEY,
        "drill/aether-data-backup-20260913-000000.json.zst.aes256gcm",
        &compressed,
    )
    .unwrap();
    let mut tampered = encrypted.clone();
    *tampered
        .last_mut()
        .expect("encrypted fixture should not be empty") ^= 1;
    write_private(&directory.join("backup.encrypted"), &tampered);
    let unauthenticated = invoke_restore(&binary, &directory, "unauthenticated.json").await;
    assert!(
        !unauthenticated.status.success(),
        "tampered backup must be rejected"
    );
    assert!(unauthenticated.stdout.is_empty());
    assert!(!directory.join("unauthenticated.json").exists());
    let untouched: i64 = sqlx::query_scalar("SELECT count(*) FROM public.users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        untouched, 0,
        "authentication failure must not modify the database"
    );
    std::fs::write(directory.join("backup.encrypted"), &encrypted).unwrap();
    let applied = invoke_restore(&binary, &directory, "restored.json").await;
    assert!(
        applied.status.success(),
        "restore failed: {}",
        String::from_utf8_lossy(&applied.stderr)
    );
    let summary: Value =
        serde_json::from_slice(&applied.stdout).expect("safe restore summary should parse");
    assert_eq!(summary["database_applied"], true);
    assert_eq!(summary["acceptance_verified"], false);
    let rows: (i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM public.users WHERE role::text <> 'admin'), (SELECT count(*) FROM public.api_keys), (SELECT count(*) FROM public.wallets), (SELECT count(*) FROM public.usage)")
        .fetch_one(&pool).await.expect("restored rows should load");
    assert_eq!(
        rows,
        (1, 1, 1, 0),
        "application backup does not include raw usage rows"
    );
    let stored_hash: String = sqlx::query_scalar(
        "SELECT password_hash FROM public.users WHERE username = 'restore-drill'",
    )
    .fetch_one(&pool)
    .await
    .expect("restored password hash should exist");
    assert!(bcrypt::verify(password, &stored_hash).expect("restored hash should verify"));
    let balance: (String, String) =
        sqlx::query_as("SELECT balance::text, gift_balance::text FROM public.wallets")
            .fetch_one(&pool)
            .await
            .expect("restored wallet should exist");
    assert_eq!(balance, ("15.00000000".into(), "5.00000000".into()));

    let state = crate::AppState::new()
        .unwrap()
        .with_data_config_and_background_isolation(
            crate::GatewayDataConfig::from_postgres_url(database_url, false)
                .with_encryption_key(DEVELOPMENT_ENCRYPTION_KEY),
            false,
        )
        .unwrap();
    let export = crate::admin_api::AdminAppState::new(&state)
        .build_admin_system_users_export_payload(crate::admin_api::SystemExportMode::RecoveryBackup)
        .await
        .expect("restored data should export through production readers");
    assert_eq!(export["users"].as_array().unwrap().len(), 1);
    assert_eq!(
        export["users"][0]["password_hash"],
        source["user_data"]["users"][0]["password_hash"]
    );
    assert_eq!(export["users"][0]["api_keys"][0]["key"], key);
    assert_eq!(
        export["usage_aggregates"],
        source["user_data"]["usage_aggregates"]
    );
    for field in [
        "balance",
        "recharge_balance",
        "gift_balance",
        "currency",
        "status",
        "limit_mode",
        "total_recharged",
        "total_consumed",
        "total_refunded",
        "total_adjusted",
    ] {
        assert_eq!(
            export["users"][0]["wallet"][field], source["user_data"]["users"][0]["wallet"][field],
            "wallet projection differs at {field}"
        );
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let auth = state
        .data
        .read_auth_api_key_snapshot_by_key_hash_strong(&key_hash, now)
        .await
        .expect("restored key lookup should run")
        .expect("original key should authenticate");
    assert!(auth.api_key_is_active);
    assert_eq!(auth.username, "restore-drill");
    let (gateway_url, server) =
        crate::tests::start_server(crate::build_router_with_state(state)).await;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();
    let login = client.post(format!("{gateway_url}/api/auth/login"))
        .header("x-client-device-id", "restore-drill-device")
        .json(&json!({"email": "restore-drill@example.com", "password": password, "auth_type": "local"}))
        .send().await.expect("original-password login should execute");
    assert_eq!(login.status(), http::StatusCode::OK);
    let logged_in: Value = login.json().await.unwrap();
    let access_token = logged_in["access_token"]
        .as_str()
        .expect("login should return an access token");
    let me = client
        .get(format!("{gateway_url}/api/auth/me"))
        .header("x-client-device-id", "restore-drill-device")
        .bearer_auth(access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), http::StatusCode::OK);
    let models = client
        .get(format!("{gateway_url}/v1/models"))
        .bearer_auth(key)
        .send()
        .await
        .expect("original API key request should execute");
    assert_eq!(models.status(), http::StatusCode::OK);
    let unauthorized = client
        .get(format!("{gateway_url}/v1/models"))
        .bearer_auth("sk-invalid-restore-drill-key")
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), http::StatusCode::UNAUTHORIZED);
    server.abort();
    // Reapplying to occupied data must fail rather than overwriting a previous exercise.
    let rejected = invoke_restore(&binary, &directory, "second.json").await;
    assert!(!rejected.status.success());
    assert!(
        rejected.stdout.is_empty(),
        "failed apply must not emit a success summary"
    );
    let unchanged: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.users WHERE username = 'restore-drill'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(unchanged, 1);
    pool.close().await;
    // Fixtures contain only synthetic data. Keep the isolated DB for evidence; remove owned secret files.
    std::fs::remove_dir_all(&directory).expect("owned synthetic fixture should be removed");
}
