use sqlx::{Connection, PgConnection, Row};

use super::{prepare_and_apply_clean_postgres_database, ManagedPostgresServer, PgPool};

const AUDIT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../docs/operations/referral-rebate-numeric-audit.sql"
));
const CAST_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../docs/operations/fixtures/referral-numeric-regression.sql"
));
const SPECIAL_VALUES_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../docs/operations/fixtures/referral-numeric-special-values.sql"
));

#[tokio::test]
async fn postgres_referral_numeric_audit_detects_missing_and_incorrect_credits() {
    let Some(server) = ManagedPostgresServer::try_start().await.unwrap() else {
        return;
    };
    let pool = PgPool::connect(server.database_url()).await.unwrap();
    prepare_and_apply_clean_postgres_database(&pool).await;
    // This data is synthetic and lives in a fresh, disposable database with
    // the real migrations. No application or production URL is accepted.
    sqlx::raw_sql(
        r#"
INSERT INTO users (id, username, email_verified)
VALUES ('audit-inviter', 'audit-inviter', false),
       ('audit-invitee', 'audit-invitee', false);
INSERT INTO user_referrals (id, inviter_user_id, invitee_user_id, invite_code_snapshot)
VALUES ('audit-referral', 'audit-inviter', 'audit-invitee', 'synthetic');
INSERT INTO wallets (id, user_id, created_at, updated_at)
VALUES ('audit-wallet', 'audit-inviter', NOW(), NOW());
INSERT INTO wallet_transactions (
  id, wallet_id, category, reason_code, amount, balance_before, balance_after,
  recharge_balance_before, recharge_balance_after, gift_balance_before,
  gift_balance_after, link_type, link_id, created_at
)
SELECT 'tx-' || reward_id, 'audit-wallet', 'adjust', reason_code, amount,
       0, 0, 0, 0, 0, 0, 'referral_reward', reward_id, NOW()
FROM (VALUES
  ('ok', 2::numeric, 'referral_reward'),
  ('quantum', 1.00000001::numeric, 'referral_reward'),
  ('debit', -2::numeric, 'referral_reward'),
  ('wrong-reason', 2::numeric, 'other')
) AS fixture(reward_id, amount, reason_code);
INSERT INTO referral_rewards (
  id, referral_id, inviter_user_id, invitee_user_id, reward_type,
  trigger_point, idempotency_key, amount_usd, status, wallet_transaction_id,
  reversed_amount_usd
)
SELECT reward_id, 'audit-referral', 'audit-inviter', 'audit-invitee', 'headcount',
       'registration', reward_id, amount, status, ledger_id, reversed
FROM (VALUES
  ('ok', 2::numeric, 'applied', 'tx-ok', 0::numeric),
  ('quantum', 1::numeric, 'applied', 'tx-quantum', 0::numeric),
  ('debit', 2::numeric, 'applied', 'tx-debit', 0::numeric),
  ('wrong-reason', 2::numeric, 'applied', 'tx-wrong-reason', 0::numeric),
  ('null-link', 3::numeric, 'applied', NULL, 0::numeric),
  ('dangling-link', 4::numeric, 'applied', 'missing-tx', 0::numeric),
  ('over-reversed', 1::numeric, 'pending', NULL, 2::numeric),
  ('nonfinite', 'NaN'::numeric, 'pending', NULL, 0::numeric)
) AS fixture(reward_id, amount, status, ledger_id, reversed);
"#,
    )
    .execute(&pool)
    .await
    .unwrap();

    let mut connection = PgConnection::connect(server.database_url()).await.unwrap();
    let rows = sqlx::raw_sql(AUDIT)
        .fetch_all(&mut connection)
        .await
        .unwrap();
    let missing = rows
        .iter()
        .find_map(|row| {
            row.try_get::<i64, _>("rows_without_wallet_transaction")
                .ok()
        })
        .unwrap();
    assert_eq!(missing, 2, "NULL and dangling links must both be visible");
    let ledger = rows
        .iter()
        .find(|row| row.try_get::<i64, _>("linked_rows").is_ok())
        .unwrap();
    assert_eq!(ledger.get::<i64, _>("linked_rows"), 4);
    assert_eq!(ledger.get::<i64, _>("amount_mismatch_rows"), 2);
    assert_eq!(ledger.get::<i64, _>("ledger_identity_mismatch_rows"), 1);
    let invalid = rows
        .iter()
        .find(|row| row.try_get::<i64, _>("nonfinite_amount_rows").is_ok())
        .unwrap();
    assert_eq!(invalid.get::<i64, _>("nonfinite_amount_rows"), 1);
    assert!(invalid.get::<i64, _>("over_reversed_rows") >= 1);
    // The report is aggregate-only even for deliberately inconsistent rows.
    for row in &rows {
        use sqlx::Column;
        assert!(row
            .columns()
            .iter()
            .all(|column| !column.name().ends_with("_id")));
    }

    let row = sqlx::query(
        "SELECT '0.00000001'::numeric(20,8) AS raw, \
         CAST('0.00000001'::numeric(20,8) AS DOUBLE PRECISION) AS decoded",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert!(row.try_get::<f64, _>("raw").is_err());
    assert_eq!(row.get::<f64, _>("decoded"), 0.00000001);

    // psql sends a file one statement at a time, unlike raw_sql's single
    // protocol message. This catches ON COMMIT DROP before INSERT when a
    // standalone fixture forgets its explicit BEGIN.
    let psql = std::env::var("AETHER_POSTGRES_BIN")
        .map(|binary| std::path::PathBuf::from(binary).with_file_name("psql"))
        .unwrap_or_else(|_| "psql".into());
    let output = std::process::Command::new(psql)
        .args(["--no-psqlrc", "--set", "ON_ERROR_STOP=1", "--dbname"])
        .arg(server.database_url())
        .arg("--file")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../docs/operations/fixtures/referral-numeric-regression.sql"
        ))
        .output()
        .expect("psql must be installed alongside the PostgreSQL server");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cast_rows = sqlx::raw_sql(CAST_FIXTURE)
        .fetch_all(&mut connection)
        .await
        .expect("standalone fixture must survive autocommit and roll back");
    let summary = cast_rows.last().unwrap();
    for column in [
        "row_count_ok",
        "min_amount_ok",
        "max_amount_ok",
        "finite_decode_input",
    ] {
        assert!(summary.get::<bool, _>(column), "{column}");
    }
    let temporary_table: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('pg_temp.referral_numeric_fixture')::text")
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert!(temporary_table.is_none());
    let special_values = sqlx::raw_sql(SPECIAL_VALUES_FIXTURE)
        .fetch_all(&mut connection)
        .await
        .expect("numeric typmod must reject infinities while preserving detectable NaN");
    assert_eq!(special_values.len(), 3);
    for row in special_values {
        let value: String = row.get("value");
        assert_eq!(
            row.get::<bool, _>("accepted_by_money_typmod"),
            value == "NaN",
            "{value}"
        );
    }
    connection.close().await.unwrap();
    pool.close().await;
}
