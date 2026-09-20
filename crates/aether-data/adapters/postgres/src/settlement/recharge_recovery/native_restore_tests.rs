//! Synthetic native PostgreSQL full-ledger backup/restore acceptance.
//! No source or restored financial receipt/job is fabricated. Historical legacy
//! usage is synthetic frozen evidence; attempt amounts come from a frozen quote.
use crate::{
    PostgresAuditLogReadRepository, SqlxProviderCostRepository, SqlxSettlementRepository,
    SqlxUsageReadRepository, SqlxWalletRepository,
};
use aether_data_contracts::repository::audit::{AuditLogWriteRepository, CreateAdminAuditLog};
use aether_data_contracts::repository::provider_cost::{
    ProviderCostCertainty, ProviderCostDimension, ProviderCostPrice,
    ProviderCostReconciliationStatus, ProviderCostRepository, ProviderCostSnapshotImport,
    ProviderCostSourceKind, ProviderCostUnit,
};
use aether_data_contracts::repository::settlement::*;
use aether_data_contracts::repository::usage::UsageBodyCaptureState;
use aether_data_contracts::repository::wallet::{
    CompleteAdminWalletRefundInput, CompleteRefundStatusNotificationInput,
    FailAdminWalletRefundInput, ProcessAdminWalletRefundInput, ProcessPaymentCallbackInput,
    ProcessPaymentCallbackOutcome, RefundNotificationOutcome, RefundStatusNotification,
    WalletMutationOutcome, WalletWriteRepository,
};
use futures_util::FutureExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{postgres::PgPoolOptions, ConnectOptions, PgPool, Row};
use std::{
    collections::BTreeMap,
    fs::File,
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

const DEBT: &str = "synthetic-prior-debt";
const LATE_DEBT: &str = "synthetic-later-debt";
const ATTEMPT: &str = "synthetic-unknown-attempt";
const UNITS_PER_IMAGE: u64 = 1_000_000;
const QUOTED_IMAGES: u64 = 8;
const RECEIPT_IMAGES: u64 = 7;
const AUDIT_PENDING: &str = "018f0000-0000-7000-8000-000000000001";
const AUDIT_RETRY: &str = "018f0000-0000-7000-8000-000000000002";
const AUDIT_DELIVERED: &str = "018f0000-0000-7000-8000-000000000003";
const AUDIT_LEASED: &str = "018f0000-0000-7000-8000-000000000004";
const AUDIT_SYSTEM_CONFIG: &str = "synthetic.audit.restore";

struct AuditRestoreFixture {
    leased_token: uuid::Uuid,
    system_config: Value,
}

#[derive(Debug, PartialEq, Eq)]
struct ProviderCostRestoreRow {
    import_id: String,
    certainty: String,
    source_kind: String,
    provider_cost_amount_units: Option<i64>,
    provider_currency: Option<String>,
    price_import_id: Option<String>,
    price_version: Option<String>,
    source_reference: Option<String>,
}

struct Harness {
    admin: PgPool,
    options: sqlx::postgres::PgConnectOptions,
    source_name: String,
    restored_name: String,
    source: Option<PgPool>,
    restored: Option<PgPool>,
    artifacts: PathBuf,
}

impl Harness {
    // No database is created until the catch_unwind/timeout scope starts.
    async fn connect() -> Self {
        let url = std::env::var("AETHER_TEST_DATABASE_URL")
            .or_else(|_| std::env::var("AETHER_TEST_POSTGRES_URL"))
            .expect("requires disposable PostgreSQL with CREATEDB permission");
        let options = url
            .parse::<sqlx::postgres::PgConnectOptions>()
            .unwrap()
            .options([("statement_timeout", "15000"), ("lock_timeout", "3000")]);
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(10))
            .connect_with(options.clone())
            .await
            .unwrap();
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let root = std::env::var_os("AETHER_NATIVE_RESTORE_ARTIFACT_DIR")
            .or_else(|| std::env::var_os("AGENT_TMP_DIR"))
            .or_else(|| std::env::var_os("RUNNER_TEMP"))
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let artifacts = root.join(format!("native-ledger-restore-{suffix}"));
        std::fs::create_dir_all(&artifacts).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&artifacts, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self {
            admin,
            options,
            source_name: format!("ledger_source_{suffix}"),
            restored_name: format!("ledger_restored_{suffix}"),
            source: None,
            restored: None,
            artifacts,
        }
    }

    async fn create(&mut self) {
        for name in [&self.source_name, &self.restored_name] {
            sqlx::query(&format!("CREATE DATABASE {name} TEMPLATE template0"))
                .execute(&self.admin)
                .await
                .unwrap();
        }
        self.source = Some(
            PgPoolOptions::new()
                .max_connections(4)
                .acquire_timeout(Duration::from_secs(10))
                .connect_with(self.options.clone().database(&self.source_name))
                .await
                .unwrap(),
        );
        crate::run_migrations(self.source.as_ref().unwrap())
            .await
            .unwrap();
        // Target stays schema-empty: native full restore owns pre/data/post-data.
    }

    async fn restore(&mut self) {
        let archive = self.artifacts.join("synthetic-ledger.dump");
        native_command(
            "dump",
            &self.artifacts,
            &self.options,
            &[
                "pg_dump",
                "--format=custom",
                "--lock-wait-timeout=5s",
                "--dbname",
                &self.source_name,
            ],
            None,
            Some(&archive),
        )
        .await;
        assert!(
            std::fs::metadata(&archive).unwrap().len() > 1024,
            "archive must not be empty"
        );
        native_command(
            "archive-list",
            &self.artifacts,
            &self.options,
            &["pg_restore", "--list"],
            Some(&archive),
            None,
        )
        .await;
        let toc = std::fs::read_to_string(self.artifacts.join("archive-list.stdout.log")).unwrap();
        for table in [
            "recharge_recovery_jobs",
            "recharge_recovery_candidates",
            "recharge_recovery_operations",
            "recharge_recovery_notifications",
            "request_fund_reservations",
            "request_fund_allocations",
            "request_fund_collection_receipts",
            "usage_cost_reservations",
            "provider_cost_prices",
            "provider_cost_snapshots",
            "provider_cost_snapshot_imports",
            "admin_audit_delivery",
            "audit_logs",
        ] {
            assert!(
                toc.lines()
                    .any(|line| line.contains("TABLE DATA") && line.contains(table)),
                "missing archive data entry {table}"
            );
        }
        assert!(toc.lines().any(
            |line| line.contains("TRIGGER") && line.contains("enqueue_recharge_debt_recovery")
        ));
        native_command(
            "restore",
            &self.artifacts,
            &self.options,
            &[
                "pg_restore",
                "--exit-on-error",
                "--single-transaction",
                "--no-owner",
                "--no-acl",
                "--dbname",
                &self.restored_name,
            ],
            Some(&archive),
            None,
        )
        .await;
        self.restored = Some(
            PgPoolOptions::new()
                .max_connections(4)
                .acquire_timeout(Duration::from_secs(10))
                .connect_with(self.options.clone().database(&self.restored_name))
                .await
                .unwrap(),
        );
        let digest = format!("{:x}", Sha256::digest(std::fs::read(&archive).unwrap()));
        std::fs::write(
            self.artifacts.join("archive.sha256"),
            format!("{digest}  synthetic-ledger.dump\n"),
        )
        .unwrap();
    }

    // Cleanup errors never replace the original test panic or timeout. FORCE is
    // limited to two generated database names and terminates an interrupted tool.
    async fn cleanup(&mut self) -> Vec<String> {
        let mut errors = Vec::new();
        for (label, pool) in [
            ("source", self.source.take()),
            ("restored", self.restored.take()),
        ] {
            if let Some(pool) = pool {
                if tokio::time::timeout(Duration::from_secs(10), pool.close())
                    .await
                    .is_err()
                {
                    errors.push(format!("{label} pool close timed out"));
                }
            }
        }
        for name in [&self.restored_name, &self.source_name] {
            let sql = format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)");
            match tokio::time::timeout(
                Duration::from_secs(20),
                sqlx::query(&sql).execute(&self.admin),
            )
            .await
            {
                Ok(Ok(_)) => (),
                Ok(Err(error)) => errors.push(format!("{name}: {error}")),
                Err(_) => errors.push(format!("{name}: cleanup timeout")),
            }
        }
        if tokio::time::timeout(Duration::from_secs(10), self.admin.close())
            .await
            .is_err()
        {
            errors.push("admin pool close timed out".into());
        }
        errors
    }
}

fn private_file(path: &Path) -> File {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).unwrap()
}

async fn copy_limited(
    reader: impl AsyncRead + Unpin,
    path: &Path,
    maximum: u64,
) -> std::io::Result<u64> {
    let mut input = reader.take(maximum + 1);
    let mut output = tokio::fs::File::from_std(private_file(path));
    let bytes = tokio::io::copy(&mut input, &mut output).await?;
    if bytes > maximum {
        return Err(std::io::Error::other(
            "native tool output exceeded its evidence bound",
        ));
    }
    Ok(bytes)
}

async fn native_command(
    label: &str,
    artifacts: &Path,
    options: &sqlx::postgres::PgConnectOptions,
    args: &[&str],
    input: Option<&Path>,
    output: Option<&Path>,
) {
    let mut command = if let Ok(container) = std::env::var("AETHER_TEST_PG_CONTAINER") {
        assert!(
            !container.trim().is_empty(),
            "explicit container must not be empty"
        );
        let mut command = Command::new("docker");
        command.args(["exec", "-i", "-e", "PGOPTIONS", "-e", "PGUSER", &container]);
        command.args(args);
        command
    } else {
        // Required CI exposes pg_config's bindir on PATH. Use native tools by
        // default; only the explicit local container switch selects docker.
        let mut command = Command::new(args[0]);
        command.args(&args[1..]);
        command.env(
            "PGHOST",
            options
                .get_socket()
                .map(|path| path.as_os_str())
                .unwrap_or_else(|| std::ffi::OsStr::new(options.get_host())),
        );
        command.env("PGPORT", options.get_port().to_string());
        // SQLx has no password getter. Decode its percent-encoded URL password
        // using URL's query decoder, then pass only an environment variable.
        // Neither credentials nor the source URL appear in argv or our logs.
        let url = options.to_url_lossy();
        if let Some(encoded) = url.password() {
            let mut decoder = url.clone();
            // Form decoding treats a literal '+' as a space; preserve it when
            // decoding a URL user-info password through query_pairs.
            decoder.set_query(Some(&format!("password={}", encoded.replace('+', "%2B"))));
            let password = decoder.query_pairs().next().unwrap().1.into_owned();
            command.env("PGPASSWORD", password);
        }
        for (key, value) in url.query_pairs() {
            let env = match key.as_ref() {
                "sslmode" | "ssl-mode" => "PGSSLMODE",
                "sslrootcert" | "ssl-root-cert" => "PGSSLROOTCERT",
                "sslcert" | "ssl-cert" => "PGSSLCERT",
                "sslkey" | "ssl-key" => "PGSSLKEY",
                _ => continue,
            };
            command.env(env, value.as_ref());
        }
        command
    };
    command.env("PGUSER", options.get_username());
    command.env(
        "PGOPTIONS",
        "-c statement_timeout=15000 -c lock_timeout=3000",
    );
    command.env("PGCONNECT_TIMEOUT", "10");
    command
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.stdin(match input {
        Some(path) => Stdio::from(File::open(path).unwrap()),
        None => Stdio::null(),
    });
    let stdout_path = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| artifacts.join(format!("{label}.stdout.log")));
    let stderr_path = artifacts.join(format!("{label}.stderr.log"));
    let mut child = command
        .spawn()
        .expect("native PostgreSQL tool should start");
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let stdout_limit = if output.is_some() {
        128 * 1024 * 1024
    } else {
        4 * 1024 * 1024
    };
    let result = tokio::time::timeout(Duration::from_secs(60), async {
        tokio::try_join!(
            child.wait(),
            copy_limited(stdout, &stdout_path, stdout_limit),
            copy_limited(stderr, &stderr_path, 2 * 1024 * 1024)
        )
    })
    .await;
    match result {
        Ok(Ok((status, _, _))) => assert!(
            status.success(),
            "{label} failed: {status}; original stderr retained at {}",
            stderr_path.display()
        ),
        Ok(Err(error)) => {
            let _ = child.kill().await;
            panic!(
                "{label} failed: {error}; original stderr retained at {}",
                stderr_path.display()
            );
        }
        Err(_) => {
            let _ = child.kill().await;
            panic!(
                "{label} exceeded 60 seconds; original stderr retained at {}",
                stderr_path.display()
            );
        }
    }
}

// Compare every ordinary public table, not merely a selected list or counts.
// Sorted JSON text keeps NUMERIC decimal values exact instead of converting f64.
async fn database_snapshot(pool: &PgPool) -> BTreeMap<String, Vec<String>> {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await
        .unwrap();
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT tablename FROM pg_tables WHERE schemaname='public' ORDER BY tablename",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    let mut snapshot = BTreeMap::new();
    for table in tables {
        assert!(table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        let rows: Vec<String> = sqlx::query_scalar(&format!(
            "SELECT to_jsonb(t)::text FROM public.\"{table}\" t ORDER BY to_jsonb(t)::text"
        ))
        .fetch_all(&mut *tx)
        .await
        .unwrap();
        snapshot.insert(table, rows);
    }
    tx.commit().await.unwrap();
    snapshot
}

async fn schema_snapshot(pool: &PgPool) -> Value {
    let constraints: Vec<(String, String, String, bool)> = sqlx::query_as("SELECT c.relname,k.conname,pg_get_constraintdef(k.oid),k.convalidated FROM pg_constraint k JOIN pg_class c ON c.oid=k.conrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' ORDER BY 1,2")
        .fetch_all(pool).await.unwrap();
    let indexes: Vec<(String, String)> = sqlx::query_as(
        "SELECT indexname,indexdef FROM pg_indexes WHERE schemaname='public' ORDER BY indexname",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    let trigger: (String, bool, bool, String) = sqlx::query_as("SELECT pg_get_triggerdef(oid),tgdeferrable,tginitdeferred,tgenabled::text FROM pg_trigger WHERE tgrelid='public.wallet_transactions'::regclass AND tgname='enqueue_recharge_debt_recovery'")
        .fetch_one(pool).await.unwrap();
    assert!(trigger.1 && trigger.2);
    assert_eq!(trigger.3, "O");
    let function: String = sqlx::query_scalar(
        "SELECT pg_get_functiondef('public.enqueue_recharge_debt_recovery()'::regprocedure)",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let sequences: Vec<String> = sqlx::query_scalar("SELECT to_jsonb(s)::text FROM pg_sequences s WHERE schemaname='public' ORDER BY sequencename")
        .fetch_all(pool).await.unwrap();
    let sequence_names: Vec<String> = sqlx::query_scalar(
        "SELECT sequencename FROM pg_sequences WHERE schemaname='public' ORDER BY sequencename",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    let mut sequence_states = BTreeMap::new();
    for name in sequence_names {
        assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        let state: (i64, bool) = sqlx::query_as(&format!(
            "SELECT last_value, is_called FROM public.\"{name}\""
        ))
        .fetch_one(pool)
        .await
        .unwrap();
        sequence_states.insert(name, state);
    }
    let views: Vec<(String, String)> = sqlx::query_as(
        "SELECT viewname,definition FROM pg_views WHERE schemaname='public' ORDER BY viewname",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    json!({"constraints":constraints,"indexes":indexes,"trigger":trigger,"function":function,"sequences":sequences,"sequence_states":sequence_states,"views":views})
}

// PostgreSQL can distribute varchar[] -> text[] coercion over constant array
// elements when reparsing pg_dump DDL. Normalize only these two spellings of
// ASCII enum literals; preserve every literal, its order and all surrounding SQL.
fn canonical_enum_array_casts(definition: &str) -> String {
    let mut sql = definition.to_string();
    for distributed in [false, true] {
        let prefix = if distributed { "ARRAY[" } else { "(ARRAY[" };
        let suffix = if distributed { "]" } else { "])::text[]" };
        let mut cursor = 0;
        while let Some(offset) = sql[cursor..].find(prefix) {
            let start = cursor + offset;
            let content = start + prefix.len();
            let Some(end_offset) = sql[content..].find(suffix) else {
                break;
            };
            let end = content + end_offset;
            let values: Option<Vec<&str>> = sql[content..end]
                .split(", ")
                .map(|item| {
                    let value = if distributed {
                        item.strip_prefix("('")?
                            .strip_suffix("'::character varying)::text")?
                    } else {
                        item.strip_prefix('\'')?
                            .strip_suffix("'::character varying")?
                    };
                    (!value.is_empty()
                        && value
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || c == b'_'))
                    .then_some(value)
                })
                .collect();
            let Some(values) = values else {
                cursor = content;
                continue;
            };
            let canonical = format!(
                "ARRAY[{}]",
                values
                    .iter()
                    .map(|value| format!("'{value}'::text"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            sql.replace_range(start..end + suffix.len(), &canonical);
            cursor = start + canonical.len();
        }
    }
    sql
}

fn comparable_schema_section(section: &str, original: &Value) -> Value {
    let mut value = original.clone();
    let definition_index = match section {
        "constraints" => 2,
        "indexes" => 1,
        _ => return value,
    };
    for row in value.as_array_mut().unwrap() {
        row[definition_index] = json!(canonical_enum_array_casts(
            row[definition_index].as_str().unwrap()
        ));
    }
    value
}

#[test]
fn schema_comparison_preserves_enum_predicates_during_native_reparse() {
    let original = "CHECK ((state)::text = ANY ((ARRAY['pending'::character varying, 'retry'::character varying])::text[]))";
    let reparsed = "CHECK ((state)::text = ANY (ARRAY[('pending'::character varying)::text, ('retry'::character varying)::text]))";
    assert_eq!(
        canonical_enum_array_casts(original),
        canonical_enum_array_casts(reparsed)
    );
    for altered in [
        reparsed.replace("'retry'", "'delivered'"),
        reparsed.replace(" = ANY ", " <> ALL "),
        reparsed.replace("(state)", "(other_column)"),
        reparsed.replace("character varying", "numeric"),
        reparsed.replace("('retry'::character varying)::text", "lower('retry')"),
    ] {
        assert_ne!(
            canonical_enum_array_casts(original),
            canonical_enum_array_casts(&altered)
        );
    }
    let non_enum = "(ARRAY['with, comma'::character varying])::text[]";
    assert_eq!(canonical_enum_array_casts(non_enum), non_enum);
}

async fn financial_snapshot(pool: &PgPool) -> Value {
    let wallet: (i64, i64, i64) = sqlx::query_as("SELECT ROUND(balance*100000000)::bigint,ROUND(gift_balance*100000000)::bigint,ROUND(total_consumed*100000000)::bigint FROM wallets WHERE id='wallet'")
        .fetch_one(pool).await.unwrap();
    let mut rows = BTreeMap::new();
    for table in [
        "recharge_recovery_jobs",
        "recharge_recovery_candidates",
        "recharge_recovery_operations",
        "request_fund_collection_receipts",
        "request_fund_recoveries",
        "request_fund_reservations",
        "request_fund_allocations",
        "usage_cost_reservations",
        "usage_daily_cost_contributions",
        "entitlement_usage_ledgers",
        "provider_cost_prices",
        "provider_cost_snapshots",
        "provider_cost_snapshot_imports",
        "wallet_transactions",
    ] {
        let values: Vec<String> = sqlx::query_scalar(&format!("SELECT (to_jsonb(t)-'updated_at')::text FROM {table} t ORDER BY (to_jsonb(t)-'updated_at')::text"))
            .fetch_all(pool).await.unwrap();
        rows.insert(table, values);
    }
    json!({"wallet":wallet,"ledger":rows})
}

async fn balances(pool: &PgPool) -> (i64, i64, i64) {
    sqlx::query_as("SELECT ROUND(balance*100000000)::bigint,ROUND(gift_balance*100000000)::bigint,ROUND(total_consumed*100000000)::bigint FROM wallets WHERE id='wallet'")
        .fetch_one(pool).await.unwrap()
}

async fn assert_ledger_links(pool: &PgPool) {
    let invalid: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM recharge_recovery_jobs j LEFT JOIN (SELECT job_id,SUM(collected_cost_units)::bigint AS total FROM recharge_recovery_operations GROUP BY job_id) o ON o.job_id=j.id WHERE j.collected_cost_units<>COALESCE(o.total,0) OR j.collected_cost_units>j.principal_cost_units")
        .fetch_one(pool).await.unwrap();
    assert_eq!(
        invalid, 0,
        "job spend must equal committed per-operation receipts and stay within budget"
    );
    let invalid: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM recharge_recovery_operations o LEFT JOIN request_fund_collection_receipts r ON r.id=o.receipt_id LEFT JOIN wallet_transactions t ON t.id=o.wallet_transaction_id WHERE r.id IS NULL OR t.id IS NULL OR o.request_id<>r.request_id OR o.collected_cost_units<>r.collected_cost_units OR ROUND(t.amount*100000000)::bigint<>-o.collected_cost_units OR t.reason_code<>'historical_debt_recovery' OR t.link_type<>'usage' OR t.link_id<>o.request_id OR t.gift_balance_before<>t.gift_balance_after")
        .fetch_one(pool).await.unwrap();
    assert_eq!(
        invalid, 0,
        "operation, collection receipt and principal-only wallet debit must agree"
    );
    let invalid: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM request_fund_recoveries r LEFT JOIN (SELECT request_id,SUM(collected_cost_units)::bigint AS total FROM request_fund_collection_receipts GROUP BY request_id) x USING(request_id) WHERE r.collected_cost_units<>COALESCE(x.total,0) OR r.collected_cost_units+r.prior_entitlement_cost_units>r.frozen_actual_cost_units")
        .fetch_one(pool).await.unwrap();
    assert_eq!(invalid, 0);
}

async fn debt(pool: &PgPool, request: &str, units: u64) {
    sqlx::query("INSERT INTO usage(id,request_id,user_id,api_key_id,provider_name,model,status,billing_status,billing_mode,actual_total_cost_usd,total_cost_usd,request_metadata) VALUES($1,$2,'owner','key-a','synthetic-provider','synthetic-image','completed','insufficient_quota','legacy',$3,$3,$4)")
        .bind(uuid::Uuid::new_v4().to_string()).bind(request).bind(request_funds_usd(units))
        .bind(json!({"settlement_snapshot":{"status":"complete","synthetic":true,"frozen_cost_units":units}}))
        .execute(pool).await.unwrap();
}

async fn credit(pool: &PgPool, units: u64) -> ProcessPaymentCallbackInput {
    let id = uuid::Uuid::new_v4().to_string();
    let order = format!("synthetic-{id}");
    let usd = request_funds_usd(units);
    sqlx::query("INSERT INTO payment_orders(id,order_no,wallet_id,user_id,amount_usd,pay_amount,pay_currency,payment_method,payment_provider,payment_channel,order_kind,status,created_at,expires_at) VALUES($1,$2,'wallet','owner',$3,$3,'USD','stripe','stripe','card','wallet_recharge','pending',now(),now()+interval '1 hour')")
        .bind(&id).bind(&order).bind(usd).execute(pool).await.unwrap();
    let input = ProcessPaymentCallbackInput {
        payment_method: "stripe".into(),
        payment_provider: Some("stripe".into()),
        payment_channel: Some("card".into()),
        callback_key: format!("stripe:synthetic-{id}"),
        order_no: Some(order),
        gateway_order_id: Some(format!("synthetic-{id}")),
        amount_usd: usd,
        pay_amount: Some(usd),
        pay_currency: Some("USD".into()),
        exchange_rate: Some(1.0),
        payload_hash: "synthetic-native-restore".into(),
        payload: json!({"synthetic":true,"status":"success"}),
        signature_valid: true,
    };
    assert!(matches!(
        SqlxWalletRepository::new(pool.clone())
            .process_payment_callback(input.clone())
            .await
            .unwrap(),
        ProcessPaymentCallbackOutcome::Applied {
            duplicate: false,
            ..
        }
    ));
    input
}

async fn seed_provider_cost_restore_fixture(pool: &PgPool) {
    let repository = SqlxProviderCostRepository::new(pool.clone());
    let price = ProviderCostPrice {
        import_id: "synthetic-native-provider-cost-price-v1".into(),
        supplier: "synthetic-native-supplier".into(),
        provider: "synthetic-provider".into(),
        model: "synthetic-image".into(),
        dimension: ProviderCostDimension::Image,
        currency: "USD".into(),
        unit: ProviderCostUnit::PerImage,
        version: "2026-09-18-v1".into(),
        price_units: 2_000_000,
        effective_from_unix_secs: 1_700_000_000,
        effective_to_unix_secs: None,
        source_reference: "synthetic-native-price-book".into(),
        imported_by: "native-restore-fixture".into(),
    };
    assert!(repository.import_price(&price).await.unwrap().inserted);
    for snapshot in [
        ProviderCostSnapshotImport {
            import_id: "synthetic-native-provider-cost-unknown".into(),
            request_id: "synthetic-native-cost-request-unknown".into(),
            provider: price.provider.clone(),
            model: price.model.clone(),
            dimension: ProviderCostDimension::Image,
            sales_amount_units: 5_000_000,
            sales_currency: "USD".into(),
            provider_cost_amount_units: None,
            provider_currency: None,
            certainty: ProviderCostCertainty::Unknown,
            source_kind: ProviderCostSourceKind::ManualImport,
            reconciliation_status: ProviderCostReconciliationStatus::NotApplicable,
            price_import_id: None,
            price_version: None,
            source_reference: None,
            price_components: Vec::new(),
            occurred_at_unix_secs: 1_700_000_100,
            imported_by: "native-restore-fixture".into(),
        },
        ProviderCostSnapshotImport {
            import_id: "synthetic-native-provider-cost-estimated".into(),
            request_id: "synthetic-native-cost-request-estimated".into(),
            provider: price.provider.clone(),
            model: price.model.clone(),
            dimension: ProviderCostDimension::Image,
            sales_amount_units: 8_000_000,
            sales_currency: "USD".into(),
            provider_cost_amount_units: Some(2_000_000),
            provider_currency: Some("USD".into()),
            certainty: ProviderCostCertainty::Estimated,
            source_kind: ProviderCostSourceKind::Estimate,
            reconciliation_status: ProviderCostReconciliationStatus::Unreconciled,
            price_import_id: Some(price.import_id.clone()),
            price_version: Some(price.version.clone()),
            source_reference: Some(price.source_reference.clone()),
            price_components: Vec::new(),
            occurred_at_unix_secs: 1_700_000_100,
            imported_by: "native-restore-fixture".into(),
        },
        ProviderCostSnapshotImport {
            import_id: "synthetic-native-provider-cost-known".into(),
            request_id: "synthetic-native-cost-request-known".into(),
            provider: price.provider.clone(),
            model: price.model.clone(),
            dimension: ProviderCostDimension::Image,
            sales_amount_units: 10_000_000,
            sales_currency: "USD".into(),
            provider_cost_amount_units: Some(3_000_000),
            provider_currency: Some("USD".into()),
            certainty: ProviderCostCertainty::Known,
            source_kind: ProviderCostSourceKind::SupplierBill,
            reconciliation_status: ProviderCostReconciliationStatus::Matched,
            price_import_id: None,
            price_version: None,
            source_reference: Some("synthetic-native-invoice-001".into()),
            price_components: Vec::new(),
            occurred_at_unix_secs: 1_700_000_100,
            imported_by: "native-restore-fixture".into(),
        },
    ] {
        assert!(
            repository
                .import_snapshot(&snapshot)
                .await
                .unwrap()
                .inserted
        );
    }
}

async fn assert_provider_cost_restore_fixture(pool: &PgPool) {
    let price: (String, i64, String, String) = sqlx::query_as(
        "SELECT import_id,price_units,version,source_reference FROM provider_cost_prices WHERE import_id='synthetic-native-provider-cost-price-v1'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        price,
        (
            "synthetic-native-provider-cost-price-v1".into(),
            2_000_000,
            "2026-09-18-v1".into(),
            "synthetic-native-price-book".into(),
        )
    );
    let snapshots = sqlx::query(
        "SELECT import_id,certainty,source_kind,provider_cost_amount_units,provider_currency,price_import_id,price_version,source_reference FROM provider_cost_snapshots WHERE import_id LIKE 'synthetic-native-provider-cost-%' ORDER BY import_id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
    .into_iter()
    .map(|row| ProviderCostRestoreRow {
        import_id: row.try_get("import_id").unwrap(),
        certainty: row.try_get("certainty").unwrap(),
        source_kind: row.try_get("source_kind").unwrap(),
        provider_cost_amount_units: row.try_get("provider_cost_amount_units").unwrap(),
        provider_currency: row.try_get("provider_currency").unwrap(),
        price_import_id: row.try_get("price_import_id").unwrap(),
        price_version: row.try_get("price_version").unwrap(),
        source_reference: row.try_get("source_reference").unwrap(),
    })
    .collect::<Vec<_>>();
    assert_eq!(
        snapshots,
        vec![
            ProviderCostRestoreRow {
                import_id: "synthetic-native-provider-cost-estimated".into(),
                certainty: "estimated".into(),
                source_kind: "estimate".into(),
                provider_cost_amount_units: Some(2_000_000),
                provider_currency: Some("USD".into()),
                price_import_id: Some("synthetic-native-provider-cost-price-v1".into()),
                price_version: Some("2026-09-18-v1".into()),
                source_reference: Some("synthetic-native-price-book".into()),
            },
            ProviderCostRestoreRow {
                import_id: "synthetic-native-provider-cost-known".into(),
                certainty: "known".into(),
                source_kind: "supplier_bill".into(),
                provider_cost_amount_units: Some(3_000_000),
                provider_currency: Some("USD".into()),
                price_import_id: None,
                price_version: None,
                source_reference: Some("synthetic-native-invoice-001".into()),
            },
            ProviderCostRestoreRow {
                import_id: "synthetic-native-provider-cost-unknown".into(),
                certainty: "unknown".into(),
                source_kind: "manual_import".into(),
                provider_cost_amount_units: None,
                provider_currency: None,
                price_import_id: None,
                price_version: None,
                source_reference: None,
            },
        ]
    );
}

fn attempt_quote() -> ReserveRequestAttemptFundsInput {
    let now = chrono::Utc::now().timestamp() as u64;
    ReserveRequestAttemptFundsInput {
        attempt_id: uuid::Uuid::new_v4().to_string(),
        provider: RequestAttemptProvider {
            provider_id: "p-a".into(),
            provider_api_key_id: Some("pk-a".into()),
            model_id: Some("image".into()),
            candidate_id: None,
        },
        quote: ReserveRequestFundsInput {
            identity: RequestFundsIdentity {
                reservation_token: "synthetic-attempt-token".into(),
                request_id: ATTEMPT.into(),
                user_id: Some("owner".into()),
                api_key_id: Some("key-a".into()),
                api_key_is_standalone: false,
            },
            authorized_cost_units: QUOTED_IMAGES * UNITS_PER_IMAGE,
            pricing_snapshot: json!({"version":1,"synthetic":true,"unit_cost_units":UNITS_PER_IMAGE,"authorized_images":QUOTED_IMAGES}),
            admitted_at_unix_secs: now,
        },
        usage_policy: Some(RequestFundsUsagePolicy {
            subject_id: "owner".into(),
            reservation_token: "synthetic-policy".into(),
            admitted_at_unix_secs: now,
            retain_until_unix_secs: now + 32 * 86400,
            windows: vec![UsagePolicyCostWindow {
                window_id: "synthetic-month".into(),
                starts_at_unix_secs: now - 60,
                ends_at_unix_secs: now + 30 * 86400,
                limit_cost_units: 20_000_000,
            }],
        }),
    }
}

fn attempt_facts(
    quote: &ReserveRequestAttemptFundsInput,
    images: Option<u64>,
) -> RecordRequestAttemptFundsOutcomeInput {
    let unit = quote.quote.pricing_snapshot["unit_cost_units"]
        .as_u64()
        .unwrap();
    RecordRequestAttemptFundsOutcomeInput {
        identity: quote.identity(),
        finalized_at_unix_secs: chrono::Utc::now().timestamp() as u64,
        facts: RequestAttemptTerminalFacts {
            schema_version: 1,
            execution: RequestAttemptExecutionFacts {
                status: RequestAttemptExecutionStatus::Failed,
                response_time_ms: 100,
            },
            outcome: images.map_or(RequestAttemptFinancialOutcome::Unknown, |n| {
                RequestAttemptFinancialOutcome::Charged {
                    usage: RequestAttemptBilledUsage {
                        total_cost_units: n * unit,
                        actual_cost_units: n * unit,
                        input_tokens: 2,
                        output_tokens: 3,
                        ..Default::default()
                    },
                }
            }),
            evidence: json!({"source":"synthetic-native-restore-receipt","output_images":images}),
        },
    }
}

async fn attempt_summary(pool: &PgPool) -> RequestFundsSummary {
    let value: Value = sqlx::query_scalar(
        "SELECT request_funds_summary FROM usage_settlement_snapshots WHERE request_id=$1",
    )
    .bind(ATTEMPT)
    .fetch_one(pool)
    .await
    .unwrap();
    serde_json::from_value(value).unwrap()
}

async fn candidates(pool: &PgPool, job: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT request_id FROM recharge_recovery_candidates WHERE job_id=$1 ORDER BY request_id",
    )
    .bind(job)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn job_for_credit(
    pool: &PgPool,
    input: &ProcessPaymentCallbackInput,
) -> StoredRechargeRecoveryJob {
    let rows = SqlxSettlementRepository::new(pool.clone())
        .list_recharge_recovery_jobs_for_user("owner", 100)
        .await
        .unwrap();
    let id: String = sqlx::query_scalar("SELECT id FROM payment_orders WHERE order_no=$1")
        .bind(input.order_no.as_deref())
        .fetch_one(pool)
        .await
        .unwrap();
    rows.into_iter()
        .find(|job| job.payment_order_id == id)
        .unwrap()
}

async fn collect_due(repo: &SqlxSettlementRepository) {
    for _ in 0..8 {
        if repo
            .process_recharge_recovery_batch(1)
            .await
            .unwrap()
            .is_empty()
        {
            return;
        }
    }
    panic!("recovery did not stop within eight operations");
}

const REFUND_WALLET: &str = "synthetic-refund-wallet";
const REFUND_DELIVERED: &str = "synthetic-refund-delivered";
const REFUND_RETRY: &str = "synthetic-refund-retry";
const REFUND_LEASED: &str = "synthetic-refund-leased";
const REFUND_PENDING: &str = "synthetic-refund-pending";

struct RefundRestoreFixture {
    delivered: RefundStatusNotification,
    retry: RefundStatusNotification,
    leased: RefundStatusNotification,
    funds: Value,
}

fn refund_complete_input(id: &str) -> CompleteAdminWalletRefundInput {
    CompleteAdminWalletRefundInput {
        wallet_id: REFUND_WALLET.into(),
        refund_id: id.into(),
        gateway_refund_id: None,
        payout_reference: None,
        payout_proof: None,
    }
}

fn refund_fail_input(id: &str) -> FailAdminWalletRefundInput {
    FailAdminWalletRefundInput {
        wallet_id: REFUND_WALLET.into(),
        refund_id: id.into(),
        reason: "synthetic offline payout failed".into(),
        operator_id: None,
    }
}

async fn refund_ack(
    repo: &SqlxWalletRepository,
    event: &RefundStatusNotification,
    outcome: RefundNotificationOutcome,
) -> bool {
    repo.complete_refund_status_notification(CompleteRefundStatusNotificationInput {
        id: event.id.clone(),
        lease_token: event.lease_token,
        outcome,
    })
    .await
    .unwrap()
}

async fn refund_funds_snapshot(pool: &PgPool) -> Value {
    let mut rows = BTreeMap::new();
    for (table, key) in [
        ("wallets", "id"),
        ("refund_requests", "wallet_id"),
        ("wallet_transactions", "wallet_id"),
    ] {
        let values: Vec<String> = sqlx::query_scalar(&format!(
            "SELECT to_jsonb(t)::text FROM {table} t WHERE {key}=$1 ORDER BY to_jsonb(t)::text"
        ))
        .bind(REFUND_WALLET)
        .fetch_all(pool)
        .await
        .unwrap();
        rows.insert(table, values);
    }
    json!(rows)
}

async fn seed_refund_restore_fixture(pool: &PgPool) -> RefundRestoreFixture {
    // Independent synthetic starting principal: real refund methods create every
    // debit/reversal and terminal event, without adding recharge recovery budgets.
    sqlx::raw_sql("INSERT INTO users(id,username,email_verified) VALUES('synthetic-refund-owner','synthetic-native-refund-owner',false); INSERT INTO wallets(id,user_id,balance,gift_balance,status,limit_mode,created_at,updated_at) VALUES('synthetic-refund-wallet','synthetic-refund-owner',0.20,0.05,'active','finite',now(),now())")
        .execute(pool).await.unwrap();
    let repo = SqlxWalletRepository::new(pool.clone());
    let mut claimed = Vec::new();
    for (id, amount, processing, succeeded) in [
        (REFUND_DELIVERED, 0.03, true, true),
        (REFUND_RETRY, 0.02, true, false),
        (REFUND_LEASED, 0.01, true, true),
        (REFUND_PENDING, 0.01, false, false),
    ] {
        sqlx::query("INSERT INTO refund_requests(id,refund_no,wallet_id,user_id,amount_usd,status,source_type,refund_mode,created_at,updated_at) VALUES($1,$1,$2,'synthetic-refund-owner',$3,'approved','wallet','offline_payout',now(),now())")
            .bind(id).bind(REFUND_WALLET).bind(amount).execute(pool).await.unwrap();
        if processing {
            assert!(matches!(
                repo.process_admin_wallet_refund(ProcessAdminWalletRefundInput {
                    wallet_id: REFUND_WALLET.into(),
                    refund_id: id.into(),
                    operator_id: None,
                })
                .await
                .unwrap(),
                WalletMutationOutcome::Applied(_)
            ));
        }
        if succeeded {
            assert!(matches!(
                repo.complete_admin_wallet_refund(refund_complete_input(id))
                    .await
                    .unwrap(),
                WalletMutationOutcome::Applied(_)
            ));
        } else {
            assert!(matches!(
                repo.fail_admin_wallet_refund(refund_fail_input(id))
                    .await
                    .unwrap(),
                WalletMutationOutcome::Applied(_)
            ));
        }
        if id == REFUND_PENDING {
            continue;
        }
        let mut events = repo.claim_refund_status_notifications(10).await.unwrap();
        assert_eq!(events.len(), 1);
        let event = events.remove(0);
        assert_eq!(event.refund_id, id);
        assert_eq!(event.wallet_id, REFUND_WALLET);
        assert_eq!(event.user_id.as_deref(), Some("synthetic-refund-owner"));
        if id == REFUND_DELIVERED {
            assert!(refund_ack(&repo, &event, RefundNotificationOutcome::Delivered).await);
        } else if id == REFUND_RETRY {
            assert!(refund_ack(&repo, &event, RefundNotificationOutcome::Retry).await);
            // Make the restored not-yet-due boundary deterministic, independent
            // of native dump duration. The full row comparison retains this due time.
            sqlx::query("UPDATE refund_status_notifications SET next_attempt_at=clock_timestamp()+interval '1 day' WHERE id=$1")
                .bind(&event.id).execute(pool).await.unwrap();
        }
        claimed.push(event);
    }
    let states: Vec<(String, String, i32, i64, bool, bool)> = sqlx::query_as("SELECT refund_id,state,attempts,lease_token,lease_until IS NOT NULL,delivered_at IS NOT NULL FROM refund_status_notifications ORDER BY refund_id")
        .fetch_all(pool).await.unwrap();
    assert_eq!(
        states,
        vec![
            (
                REFUND_DELIVERED.into(),
                "delivered".into(),
                0,
                1,
                false,
                true
            ),
            (REFUND_LEASED.into(), "pending".into(), 0, 1, true, false),
            (REFUND_PENDING.into(), "pending".into(), 0, 0, false, false),
            (REFUND_RETRY.into(), "retry".into(), 1, 1, false, false),
        ]
    );
    let totals: (i64, i64, i64, i64) = sqlx::query_as("SELECT ROUND(balance*100000000)::bigint,ROUND(gift_balance*100000000)::bigint,ROUND(total_refunded*100000000)::bigint,(SELECT COUNT(*) FROM wallet_transactions WHERE wallet_id=$1) FROM wallets WHERE id=$1")
        .bind(REFUND_WALLET).fetch_one(pool).await.unwrap();
    assert_eq!(totals, (16_000_000, 5_000_000, 4_000_000, 4));
    let mut claims = claimed.into_iter();
    RefundRestoreFixture {
        delivered: claims.next().unwrap(),
        retry: claims.next().unwrap(),
        leased: claims.next().unwrap(),
        funds: refund_funds_snapshot(pool).await,
    }
}

async fn replay_restored_refund_notifications(pool: &PgPool, fixture: &RefundRestoreFixture) {
    let repo = SqlxWalletRepository::new(pool.clone());
    let all_money = financial_snapshot(pool).await;
    assert_eq!(refund_funds_snapshot(pool).await, fixture.funds);
    // The full database snapshot already checked exact due times, lease tokens,
    // lease deadlines, attempts and delivery timestamps before any target writes.
    let gates: (bool, bool) = sqlx::query_as("SELECT (SELECT next_attempt_at>clock_timestamp() FROM refund_status_notifications WHERE id=$1),(SELECT lease_until>clock_timestamp() FROM refund_status_notifications WHERE id=$2)")
        .bind(&fixture.retry.id).bind(&fixture.leased.id).fetch_one(pool).await.unwrap();
    assert_eq!(gates, (true, true));
    let mut first = repo.claim_refund_status_notifications(10).await.unwrap();
    assert_eq!(
        first.len(),
        1,
        "restored due and lease gates exclude retry, delivered and active lease"
    );
    let pending = first.remove(0);
    assert_eq!(pending.refund_id, REFUND_PENDING);
    assert_eq!(pending.lease_token, 1);
    assert!(refund_ack(&repo, &pending, RefundNotificationOutcome::Delivered).await);
    assert!(!refund_ack(&repo, &pending, RefundNotificationOutcome::Delivered).await);
    assert!(
        !refund_ack(
            &repo,
            &fixture.delivered,
            RefundNotificationOutcome::Delivered
        )
        .await
    );
    assert!(!refund_ack(&repo, &fixture.retry, RefundNotificationOutcome::Delivered).await);
    assert_eq!(financial_snapshot(pool).await, all_money);
    assert_eq!(refund_funds_snapshot(pool).await, fixture.funds);

    sqlx::query("UPDATE refund_status_notifications SET next_attempt_at=clock_timestamp()-interval '1 second',lease_until=CASE WHEN id=$2 THEN clock_timestamp()-interval '1 second' ELSE lease_until END WHERE id IN ($1,$2)")
        .bind(&fixture.retry.id).bind(&fixture.leased.id).execute(pool).await.unwrap();
    assert!(!refund_ack(&repo, &fixture.leased, RefundNotificationOutcome::Delivered).await);
    let events = repo.claim_refund_status_notifications(10).await.unwrap();
    assert_eq!(events.len(), 2);
    for event in events {
        let old = if event.id == fixture.retry.id {
            &fixture.retry
        } else {
            assert_eq!(event.id, fixture.leased.id);
            &fixture.leased
        };
        assert!(event.lease_token > old.lease_token);
        assert!(!refund_ack(&repo, old, RefundNotificationOutcome::Retry).await);
        assert!(refund_ack(&repo, &event, RefundNotificationOutcome::Delivered).await);
        assert!(!refund_ack(&repo, &event, RefundNotificationOutcome::Delivered).await);
    }
    assert!(repo
        .claim_refund_status_notifications(10)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(financial_snapshot(pool).await, all_money);
    assert_eq!(refund_funds_snapshot(pool).await, fixture.funds);
    let delivered: (i64, i64, i64) = sqlx::query_as("SELECT COUNT(*),SUM(attempts)::bigint,COUNT(DISTINCT refund_id) FROM refund_status_notifications WHERE state='delivered' AND lease_until IS NULL AND delivered_at IS NOT NULL")
        .fetch_one(pool).await.unwrap();
    assert_eq!(delivered, (4, 1, 4));

    let before_replay = database_snapshot(pool).await;
    for id in [REFUND_DELIVERED, REFUND_LEASED] {
        assert!(matches!(
            repo.complete_admin_wallet_refund(refund_complete_input(id))
                .await
                .unwrap(),
            WalletMutationOutcome::Applied(_)
        ));
    }
    for id in [REFUND_RETRY, REFUND_PENDING] {
        assert!(matches!(
            repo.fail_admin_wallet_refund(refund_fail_input(id))
                .await
                .unwrap(),
            WalletMutationOutcome::Invalid(_)
        ));
    }
    assert_eq!(
        database_snapshot(pool).await,
        before_replay,
        "terminal refund replay must not debit, reverse, append events or reopen delivery"
    );
    assert_ledger_links(pool).await;
}

fn audit_record(id: &str, status: &str) -> CreateAdminAuditLog {
    CreateAdminAuditLog {
        id: id.to_string(),
        event_type: "admin_mutation".to_string(),
        user_id: None,
        api_key_id: None,
        description: "admin action: update_system_config".to_string(),
        ip_address: Some("127.0.0.1".to_string()),
        user_agent: None,
        request_id: Some(id.to_string()),
        event_metadata: Some(json!({
            "schema_version": 1,
            "event_name": "admin_system_config_updated",
            "status": status,
            "admin_role": "admin",
            "route_family": "system_manage",
            "route_kind": "config_set",
            "method": "PUT",
            "path": "/api/admin/system/configs/[key]",
            "action": "update_system_config",
            "target_type": "system_config",
            "target_id": AUDIT_SYSTEM_CONFIG
        })),
        status_code: Some(200),
        error_message: None,
        created_at: chrono::Utc::now(),
    }
}

async fn seed_audit_restore_fixture(pool: &PgPool) -> AuditRestoreFixture {
    let system_config = json!({"enabled": true, "generation": 7});
    let records = [
        audit_record(AUDIT_PENDING, "completed"),
        audit_record(AUDIT_RETRY, "completed"),
        audit_record(AUDIT_DELIVERED, "completed"),
        audit_record(AUDIT_LEASED, "completed"),
    ];
    let leased_token = uuid::Uuid::new_v4();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO system_configs(id,key,value,description,created_at,updated_at)
         VALUES($1,$2,$3,'native audit restore fixture',clock_timestamp(),clock_timestamp())",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(AUDIT_SYSTEM_CONFIG)
    .bind(&system_config)
    .execute(&mut *tx)
    .await
    .unwrap();
    for record in &records {
        record.validate().unwrap();
        sqlx::query(
            "INSERT INTO admin_audit_delivery(event_id,payload)
             VALUES($1,$2)",
        )
        .bind(&record.id)
        .bind(serde_json::to_value(record).unwrap())
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    sqlx::query(
        "UPDATE admin_audit_delivery
         SET attempt_count=2,
             next_attempt_at=clock_timestamp()+interval '1 day',
             last_error_code='audit_insert_failed',
             updated_at=clock_timestamp()
         WHERE event_id=$1",
    )
    .bind(AUDIT_RETRY)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE admin_audit_delivery
         SET state='leased',lease_token=$2,
             lease_expires_at=clock_timestamp()+interval '1 day',
             updated_at=clock_timestamp()
         WHERE event_id=$1",
    )
    .bind(AUDIT_LEASED)
    .bind(leased_token)
    .execute(&mut *tx)
    .await
    .unwrap();
    // Delivered and late-commit rows already have their immutable audit fact.
    // The pending late-commit row proves ON CONFLICT delivery can ACK without
    // appending a second fact after restore.
    for record in [&records[2], &records[0]] {
        sqlx::query(
            "INSERT INTO audit_logs(
               id,event_type,user_id,api_key_id,description,ip_address,user_agent,
               request_id,event_metadata,status_code,error_message,created_at
             ) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
        )
        .bind(&record.id)
        .bind(&record.event_type)
        .bind(record.user_id.as_deref())
        .bind(record.api_key_id.as_deref())
        .bind(&record.description)
        .bind(record.ip_address.as_deref())
        .bind(record.user_agent.as_deref())
        .bind(record.request_id.as_deref())
        .bind(record.event_metadata.clone())
        .bind(record.status_code)
        .bind(record.error_message.as_deref())
        .bind(record.created_at)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    sqlx::query(
        "UPDATE admin_audit_delivery
         SET state='delivered',delivered_at=clock_timestamp(),
             updated_at=clock_timestamp()
         WHERE event_id=$1",
    )
    .bind(AUDIT_DELIVERED)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    AuditRestoreFixture {
        leased_token,
        system_config,
    }
}

async fn replay_restored_audit_delivery(pool: &PgPool, fixture: &AuditRestoreFixture) {
    let repository = PostgresAuditLogReadRepository::new(pool.clone());
    let config_before: Value =
        sqlx::query_scalar("SELECT row_to_json(c)::jsonb FROM system_configs c WHERE key=$1")
            .bind(AUDIT_SYSTEM_CONFIG)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(config_before["value"], fixture.system_config);
    let initial: Vec<(String, String, i32, bool, bool)> = sqlx::query_as(
        "SELECT event_id,state,attempt_count,lease_token IS NOT NULL,delivered_at IS NOT NULL
         FROM admin_audit_delivery ORDER BY event_id",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(
        initial,
        vec![
            (AUDIT_PENDING.into(), "pending".into(), 0, false, false),
            (AUDIT_RETRY.into(), "pending".into(), 2, false, false),
            (AUDIT_DELIVERED.into(), "delivered".into(), 0, false, true),
            (AUDIT_LEASED.into(), "leased".into(), 0, true, false),
        ]
    );
    let pending = repository
        .claim_admin_audit_deliveries(1, 30)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(pending.event_id, AUDIT_PENDING);
    assert!(repository
        .deliver_admin_audit(&pending.event_id, pending.lease_token)
        .await
        .unwrap());
    assert!(!repository
        .deliver_admin_audit(&pending.event_id, pending.lease_token)
        .await
        .unwrap());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_logs WHERE id=$1")
            .bind(AUDIT_PENDING)
            .fetch_one(pool)
            .await
            .unwrap(),
        1,
        "late commit replay must ACK the same event id without duplicate audit facts"
    );
    assert!(repository
        .claim_admin_audit_deliveries(1, 30)
        .await
        .unwrap()
        .is_empty());
    sqlx::query(
        "UPDATE admin_audit_delivery
         SET next_attempt_at=CASE WHEN event_id=$1 THEN clock_timestamp() ELSE next_attempt_at END,
             lease_expires_at=CASE WHEN event_id=$2 THEN clock_timestamp()-interval '1 second'
                                   ELSE lease_expires_at END
         WHERE event_id IN ($1,$2)",
    )
    .bind(AUDIT_RETRY)
    .bind(AUDIT_LEASED)
    .execute(pool)
    .await
    .unwrap();
    assert!(!repository
        .deliver_admin_audit(AUDIT_LEASED, fixture.leased_token)
        .await
        .unwrap());
    let claims = repository
        .claim_admin_audit_deliveries(2, 30)
        .await
        .unwrap();
    assert_eq!(claims.len(), 2);
    for claim in claims {
        if claim.event_id == AUDIT_LEASED {
            assert_ne!(claim.lease_token, fixture.leased_token);
        } else {
            assert_eq!(claim.event_id, AUDIT_RETRY);
            assert_eq!(claim.attempt_count, 2);
        }
        assert!(repository
            .deliver_admin_audit(&claim.event_id, claim.lease_token)
            .await
            .unwrap());
    }
    assert!(repository
        .claim_admin_audit_deliveries(4, 30)
        .await
        .unwrap()
        .is_empty());
    let terminal: (i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT COUNT(*) FROM admin_audit_delivery WHERE state='delivered'),
           (SELECT COUNT(DISTINCT id) FROM audit_logs
            WHERE id IN ($1,$2,$3,$4))",
    )
    .bind(AUDIT_PENDING)
    .bind(AUDIT_RETRY)
    .bind(AUDIT_DELIVERED)
    .bind(AUDIT_LEASED)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(terminal, (4, 4));
    let config_after: Value =
        sqlx::query_scalar("SELECT row_to_json(c)::jsonb FROM system_configs c WHERE key=$1")
            .bind(AUDIT_SYSTEM_CONFIG)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        config_after, config_before,
        "audit redelivery must not replay the system-config business mutation"
    );
}

async fn exercise(h: &mut Harness) {
    h.create().await;
    native_command(
        "pg-dump-version",
        &h.artifacts,
        &h.options,
        &["pg_dump", "--version"],
        None,
        None,
    )
    .await;
    native_command(
        "pg-restore-version",
        &h.artifacts,
        &h.options,
        &["pg_restore", "--version"],
        None,
        None,
    )
    .await;
    let source = h.source.as_ref().unwrap().clone();
    sqlx::raw_sql("INSERT INTO users(id,username,email_verified) VALUES('owner','synthetic-native-restore-owner',false); INSERT INTO api_keys(id,user_id,key_hash,name) VALUES('key-a','owner','synthetic-hash','synthetic'); INSERT INTO wallets(id,user_id,balance,gift_balance,status,limit_mode,created_at,updated_at) VALUES('wallet','owner',0.20,0.03,'active','finite',now(),now()); INSERT INTO providers(id,name,provider_type,created_at,updated_at) VALUES('p-a','synthetic-provider','custom',now(),now()); INSERT INTO provider_api_keys(id,provider_id,name,api_key,total_tokens,total_cost_usd,created_at,updated_at) VALUES('pk-a','p-a','synthetic','synthetic-secret',0,0,now(),now())")
        .execute(&source).await.unwrap();
    seed_provider_cost_restore_fixture(&source).await;
    let mut parent = crate::usage::tests::fast_clear_usage_record(
        ATTEMPT,
        "synthetic-provider",
        chrono::Utc::now().timestamp() as u64,
        false,
        UsageBodyCaptureState::None,
        None,
    );
    parent.user_id = Some("owner".into());
    parent.api_key_id = Some("key-a".into());
    parent.provider_id = Some("p-a".into());
    parent.provider_api_key_id = Some("pk-a".into());
    SqlxUsageReadRepository::new(source.clone())
        .upsert(parent)
        .await
        .unwrap();
    let server_major: i32 =
        sqlx::query_scalar("SELECT current_setting('server_version_num')::integer / 10000")
            .fetch_one(&source)
            .await
            .unwrap();
    for tool in ["pg-dump-version", "pg-restore-version"] {
        let version =
            std::fs::read_to_string(h.artifacts.join(format!("{tool}.stdout.log"))).unwrap();
        let major = version
            .split_whitespace()
            .find_map(|token| token.split('.').next().unwrap().parse::<i32>().ok())
            .unwrap();
        assert_eq!(
            major, server_major,
            "native tool and database major versions must match"
        );
    }
    let repo = SqlxSettlementRepository::new(source.clone());
    let quote = attempt_quote();
    assert!(matches!(
        repo.reserve_request_attempt_funds(quote.clone())
            .await
            .unwrap(),
        ReserveRequestAttemptFundsOutcome::Reserved { .. }
    ));
    repo.mark_request_attempt_funds_dispatched(quote.identity())
        .await
        .unwrap();
    let unknown = attempt_facts(&quote, None);
    repo.record_request_attempt_funds_outcome(unknown.clone())
        .await
        .unwrap()
        .unwrap();
    repo.close_request_funds_admission(CloseRequestFundsAdmissionInput {
        identity: quote.identity(),
        closed_at_unix_secs: unknown.finalized_at_unix_secs,
    })
    .await
    .unwrap()
    .unwrap();
    let frozen = attempt_summary(&source).await;
    assert_eq!(frozen.held_cost_units, 8_000_000);
    assert_eq!(frozen.unknown_attempts, 1);
    debt(&source, DEBT, 20_000_000).await;
    let first_credit = credit(&source, 5_000_000).await;
    collect_due(&repo).await;
    let first_job = job_for_credit(&source, &first_credit).await;
    assert_eq!(first_job.state, "waiting_next_recharge");
    assert_eq!(first_job.collected_cost_units, 5_000_000);
    let failed_claim = repo
        .claim_recharge_recovery_notifications(1)
        .await
        .unwrap()
        .remove(0);
    assert!(repo
        .complete_recharge_recovery_notification(CompleteRechargeRecoveryNotificationInput {
            id: failed_claim.id.clone(),
            lease_token: failed_claim.lease_token,
            outcome: RechargeRecoveryNotificationOutcome::Retry,
            error_code: Some("synthetic_transport_failure".into())
        })
        .await
        .unwrap());
    let second_credit = credit(&source, 4_000_000).await;
    let second_job = job_for_credit(&source, &second_credit).await;
    assert_eq!(second_job.state, "pending");
    assert_eq!(second_job.collected_cost_units, 0);
    debt(&source, LATE_DEBT, 4_000_000).await;
    assert_eq!(candidates(&source, &second_job.id).await, vec![DEBT]);
    assert_eq!(attempt_summary(&source).await, frozen);
    assert_eq!(balances(&source).await, (24_000_000, 3_000_000, 5_000_000));
    assert_ledger_links(&source).await;
    let refunds = seed_refund_restore_fixture(&source).await;
    let audit_delivery = seed_audit_restore_fixture(&source).await;
    assert_eq!(balances(&source).await, (24_000_000, 3_000_000, 5_000_000));
    assert_ledger_links(&source).await;
    let before = database_snapshot(&source).await;
    for table in [
        "recharge_recovery_jobs",
        "recharge_recovery_candidates",
        "recharge_recovery_operations",
        "recharge_recovery_notifications",
        "request_fund_reservations",
        "request_fund_allocations",
        "request_fund_recoveries",
        "request_fund_collection_receipts",
        "usage_cost_reservations",
        "usage_settlement_snapshots",
        "payment_callbacks",
        "refund_requests",
        "refund_status_notifications",
        "admin_audit_delivery",
        "audit_logs",
        "system_configs",
    ] {
        assert!(
            !before[table].is_empty(),
            "required nonempty ledger table {table}"
        );
    }
    let schema_before = schema_snapshot(&source).await;
    std::fs::write(
        h.artifacts.join("source-schema.json"),
        serde_json::to_vec_pretty(&schema_before).unwrap(),
    )
    .unwrap();
    std::fs::write(
        h.artifacts.join("source-ledger.json"),
        serde_json::to_vec_pretty(&before).unwrap(),
    )
    .unwrap();
    h.restore().await;
    let restored = h.restored.as_ref().unwrap().clone();
    let after = database_snapshot(&restored).await;
    std::fs::write(
        h.artifacts.join("restored-ledger.json"),
        serde_json::to_vec_pretty(&after).unwrap(),
    )
    .unwrap();
    assert_eq!(
        after, before,
        "every public table must survive native restore byte-for-byte as canonical JSON rows"
    );
    assert_provider_cost_restore_fixture(&restored).await;
    let schema_after = schema_snapshot(&restored).await;
    std::fs::write(
        h.artifacts.join("restored-schema.json"),
        serde_json::to_vec_pretty(&schema_after).unwrap(),
    )
    .unwrap();
    for (section, original) in schema_before.as_object().unwrap() {
        assert!(
            comparable_schema_section(section, &schema_after[section])
                == comparable_schema_section(section, original),
            "schema section {section} must survive; inspect source-schema.json and restored-schema.json"
        );
    }
    crate::run_migrations(&restored).await.unwrap();
    assert_eq!(
        database_snapshot(&restored).await,
        before,
        "current migration runner must be a no-op on restored history"
    );
    assert_ledger_links(&restored).await;
    replay_restored_refund_notifications(&restored, &refunds).await;
    replay_restored_audit_delivery(&restored, &audit_delivery).await;
    let restored_repo = SqlxSettlementRepository::new(restored.clone());
    let restored_wallet = SqlxWalletRepository::new(restored.clone());
    let baseline = financial_snapshot(&restored).await;
    for callback in [&first_credit, &second_credit] {
        assert!(matches!(
            restored_wallet
                .process_payment_callback(callback.clone())
                .await
                .unwrap(),
            ProcessPaymentCallbackOutcome::DuplicateProcessed { .. }
                | ProcessPaymentCallbackOutcome::AlreadyCredited { .. }
        ));
    }
    assert_eq!(financial_snapshot(&restored).await, baseline);
    assert!(
        !restored_repo
            .complete_recharge_recovery_notification(CompleteRechargeRecoveryNotificationInput {
                id: failed_claim.id.clone(),
                lease_token: failed_claim.lease_token,
                outcome: RechargeRecoveryNotificationOutcome::Delivered,
                error_code: None
            })
            .await
            .unwrap(),
        "failed pre-backup lease cannot be acknowledged after restore"
    );
    collect_due(&restored_repo).await;
    assert_eq!(
        job_for_credit(&restored, &first_credit)
            .await
            .collected_cost_units,
        5_000_000
    );
    assert_eq!(
        job_for_credit(&restored, &second_credit)
            .await
            .collected_cost_units,
        4_000_000
    );
    assert_eq!(candidates(&restored, &second_job.id).await, vec![DEBT]);
    assert_eq!(
        balances(&restored).await,
        (20_000_000, 3_000_000, 9_000_000)
    );
    assert_eq!(
        attempt_summary(&restored).await,
        frozen,
        "recharge must not spend the restored unknown hold"
    );
    let late_status: String =
        sqlx::query_scalar("SELECT billing_status FROM usage WHERE request_id=$1")
            .bind(LATE_DEBT)
            .fetch_one(&restored)
            .await
            .unwrap();
    assert_eq!(late_status, "insufficient_quota");
    let collected: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM request_fund_collection_receipts WHERE request_id=$1",
    )
    .bind(LATE_DEBT)
    .fetch_one(&restored)
    .await
    .unwrap();
    assert_eq!(
        collected, 0,
        "pending pre-backup budget cannot capture post-credit debt"
    );
    let stable = financial_snapshot(&restored).await;
    collect_due(&restored_repo).await;
    assert_eq!(financial_snapshot(&restored).await, stable);
    let retry: (String, i32) =
        sqlx::query_as("SELECT state,attempts FROM recharge_recovery_notifications WHERE id=$1")
            .bind(&failed_claim.id)
            .fetch_one(&restored)
            .await
            .unwrap();
    assert_eq!(retry, ("retry".into(), 1));
    sqlx::query(
        "UPDATE recharge_recovery_notifications SET next_attempt_at=clock_timestamp() WHERE id=$1",
    )
    .bind(&failed_claim.id)
    .execute(&restored)
    .await
    .unwrap();
    let claims = restored_repo
        .claim_recharge_recovery_notifications(10)
        .await
        .unwrap();
    assert!(claims
        .iter()
        .any(|claim| claim.id == failed_claim.id && claim.lease_token > failed_claim.lease_token));
    for claim in claims {
        let ack = CompleteRechargeRecoveryNotificationInput {
            id: claim.id,
            lease_token: claim.lease_token,
            outcome: RechargeRecoveryNotificationOutcome::Delivered,
            error_code: None,
        };
        assert!(restored_repo
            .complete_recharge_recovery_notification(ack.clone())
            .await
            .unwrap());
        assert!(!restored_repo
            .complete_recharge_recovery_notification(ack)
            .await
            .unwrap());
    }
    assert_eq!(
        financial_snapshot(&restored).await,
        stable,
        "notification retry/ACK must not mutate money"
    );
    let stored_quote: Value = sqlx::query_scalar(
        "SELECT quote FROM request_fund_reservations WHERE reservation_token=$1",
    )
    .bind(&quote.quote.identity.reservation_token)
    .fetch_one(&restored)
    .await
    .unwrap();
    assert_eq!(
        stored_quote["pricing_snapshot"],
        quote.quote.pricing_snapshot
    );
    let late = attempt_facts(&quote, Some(RECEIPT_IMAGES));
    restored_repo
        .record_request_attempt_funds_outcome(late.clone())
        .await
        .unwrap()
        .unwrap();
    let reconciled = attempt_summary(&restored).await;
    assert_eq!(reconciled.held_cost_units, 0);
    assert_eq!(reconciled.unknown_attempts, 0);
    assert_eq!(
        reconciled.known_actual_cost_units,
        RECEIPT_IMAGES * UNITS_PER_IMAGE
    );
    let balance = balances(&restored).await;
    assert_eq!(balance.0 + balance.1, 16_000_000);
    assert_eq!(balance.2, 16_000_000);
    let quota:(String,i64)=sqlx::query_as("SELECT state,actual_cost_units FROM usage_cost_reservations WHERE attempt_reservation_token=$1").bind(&quote.quote.identity.reservation_token).fetch_one(&restored).await.unwrap();
    assert_eq!(quota, ("finalized".into(), 7_000_000));
    let final_attempt = financial_snapshot(&restored).await;
    restored_repo
        .record_request_attempt_funds_outcome(late)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        restored_repo
            .reserve_request_attempt_funds(quote.clone())
            .await
            .unwrap(),
        ReserveRequestAttemptFundsOutcome::Reserved { .. }
    ));
    assert_eq!(
        financial_snapshot(&restored).await,
        final_attempt,
        "late receipt and reservation replay must not debit again"
    );
    let third_credit = credit(&restored, 12_000_000).await;
    let third_job = job_for_credit(&restored, &third_credit).await;
    let third_candidates = candidates(&restored, &third_job.id).await;
    assert_eq!(third_candidates, vec![LATE_DEBT, DEBT]);
    collect_due(&restored_repo).await;
    let order:Vec<String>=sqlx::query_scalar("SELECT request_id FROM recharge_recovery_operations WHERE job_id=$1 ORDER BY operation_seq").bind(&third_job.id).fetch_all(&restored).await.unwrap();
    assert_eq!(order, vec![DEBT, LATE_DEBT]);
    assert_eq!(
        job_for_credit(&restored, &third_credit)
            .await
            .collected_cost_units,
        12_000_000
    );
    assert_eq!(
        job_for_credit(&restored, &first_credit)
            .await
            .collected_cost_units,
        5_000_000
    );
    assert_eq!(
        job_for_credit(&restored, &second_credit)
            .await
            .collected_cost_units,
        4_000_000
    );
    let now_balance = balances(&restored).await;
    assert_eq!(now_balance.0 + now_balance.1, 16_000_000);
    assert_eq!(now_balance.1, balance.1);
    assert_eq!(now_balance.2, 28_000_000);
    assert_ledger_links(&restored).await;
    assert_eq!(
        refund_funds_snapshot(&restored).await,
        refunds.funds,
        "primary recharge and receipt replay must leave the independent refund branch unchanged"
    );
    let final_money = financial_snapshot(&restored).await;
    restored_wallet
        .process_payment_callback(third_credit)
        .await
        .unwrap();
    collect_due(&restored_repo).await;
    assert_eq!(financial_snapshot(&restored).await, final_money);
    assert_eq!(
        database_snapshot(&source).await,
        before,
        "restore/replay must never mutate the source database"
    );
    std::fs::write(h.artifacts.join("reconciliation.json"),serde_json::to_vec_pretty(&json!({"fixture":"synthetic","reconciliation_verified":true,"source_database":h.source_name,"restored_database":h.restored_name,"public_tables_compared":before.len(),"callbacks_replayed":3,"refund_events_restored":4,"refund_delivery_states_restored":["delivered","retry","pending"],"refund_active_lease_restored":true,"refund_terminal_replays":4,"refund_notifications_verified":true,"audit_delivery_events_restored":4,"audit_delivery_states_restored":["pending","retry","delivered","leased"],"audit_active_lease_restored":true,"audit_late_commit_deduplicated":true,"audit_business_replay_count":0,"refund_principal_units":16_000_000,"refund_total_refunded_units":4_000_000,"recovery_collected_units":21_000_000,"attempt_actual_units":7_000_000,"wallet_total_units":16_000_000})).unwrap()).unwrap();
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and matching native pg_dump/pg_restore or explicit AETHER_TEST_PG_CONTAINER; removes two dedicated databases"]
async fn live_native_postgres_restore_preserves_nonempty_funds_and_replay_boundaries() {
    let mut h = Harness::connect().await;
    eprintln!(
        "synthetic native restore evidence: {}",
        h.artifacts.display()
    );
    let result = AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(240),
        exercise(&mut h),
    ))
    .catch_unwind()
    .await;
    let cleanup = h.cleanup().await;
    if !cleanup.is_empty() {
        eprintln!("native restore cleanup errors: {cleanup:?}");
    }
    match result {
        Err(original) => std::panic::resume_unwind(original),
        Ok(Err(_)) => {
            panic!("native ledger restore exceeded 240 seconds; cleanup errors: {cleanup:?}")
        }
        Ok(Ok(())) => {
            assert!(
                cleanup.is_empty(),
                "native restore passed but cleanup failed: {cleanup:?}"
            );
            let mut verified: Value = serde_json::from_slice(
                &std::fs::read(h.artifacts.join("reconciliation.json")).unwrap(),
            )
            .unwrap();
            verified["native_restore_verified"] = json!(true);
            verified["dedicated_databases_removed"] = json!(true);
            std::fs::write(
                h.artifacts.join("verified.json"),
                serde_json::to_vec_pretty(&verified).unwrap(),
            )
            .unwrap();
        }
    }
}
