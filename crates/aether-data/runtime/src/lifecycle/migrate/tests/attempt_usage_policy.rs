use super::{ManagedPostgresServer, PgPool, POSTGRES_MIGRATOR};

#[tokio::test]
async fn attempt_usage_policy_migration_ignores_unrelated_constraint_names() {
    let Some(server) = ManagedPostgresServer::try_start()
        .await
        .expect("postgres fixture should start")
    else {
        return;
    };
    let pool = PgPool::connect(server.database_url())
        .await
        .expect("postgres pool should connect");
    sqlx::raw_sql(include_str!("attempt_usage_policy_fixture.sql"))
        .execute(&pool)
        .await
        .expect("pre-migration tables and unrelated constraints should be created");
    let migration = POSTGRES_MIGRATOR
        .iter()
        .find(|migration| migration.version == 20260914020000)
        .expect("attempt usage policy migration should be embedded");

    // First apply upgrades the legacy tables; reapplication checks compatibility
    // with a bootstrap that already includes the columns and constraints.
    for _ in 0..2 {
        sqlx::raw_sql(&migration.sql)
            .execute(&pool)
            .await
            .expect("attempt policy migration should apply idempotently");
    }
    for (table, name) in [
        (
            "public.request_fund_reservations",
            "request_fund_reservations_usage_policy_check",
        ),
        (
            "public.usage_cost_reservations",
            "usage_cost_reservations_attempt_token_check",
        ),
        (
            "public.usage_cost_reservations",
            "usage_cost_reservations_attempt_token_fkey",
        ),
    ] {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_constraint WHERE conrelid = to_regclass($1) AND conname = $2)",
        )
        .bind(table)
        .bind(name)
        .fetch_one(&pool)
        .await
        .expect("target constraint should be readable");
        assert!(exists, "missing {name} on {table}");
    }

    sqlx::query(
        "INSERT INTO public.request_fund_reservations (reservation_token, attempt_id, usage_policy) \
         VALUES ('attempt', '00000000-0000-4000-8000-000000000001', '{}'::jsonb)",
    )
    .execute(&pool)
    .await
    .expect("valid attempt policy should be accepted");
    sqlx::query(
        "INSERT INTO public.usage_cost_reservations (reservation_token, attempt_reservation_token) \
         VALUES ('attempt', 'attempt')",
    )
    .execute(&pool)
    .await
    .expect("quota should reference its own existing attempt");

    for (statement, code) in [
        (
            "UPDATE public.request_fund_reservations SET usage_policy = '{}'::jsonb WHERE reservation_token = 'legacy'",
            "23514",
        ),
        (
            "UPDATE public.request_fund_reservations SET usage_policy = '[]'::jsonb WHERE reservation_token = 'attempt'",
            "23514",
        ),
        (
            "UPDATE public.usage_cost_reservations SET attempt_reservation_token = 'attempt' WHERE reservation_token = 'legacy'",
            "23514",
        ),
        (
            "INSERT INTO public.usage_cost_reservations VALUES ('missing', 'missing')",
            "23503",
        ),
        (
            "DELETE FROM public.request_fund_reservations WHERE reservation_token = 'attempt'",
            "23503",
        ),
    ] {
        let error = sqlx::query(statement)
            .execute(&pool)
            .await
            .expect_err("invalid financial linkage must be rejected");
        assert_eq!(
            error.as_database_error().and_then(|error| error.code()).as_deref(),
            Some(code),
            "unexpected failure for {statement}: {error}"
        );
    }
    pool.close().await;
}
