use super::*;
use aether_data_contracts::repository::usage::{DailyActualCostQuery, UsageReadRepository};

fn window(at: u64) -> DailyActualCostQuery {
    DailyActualCostQuery {
        user_id: Some("owner".into()),
        api_key_id: "key-a".into(),
        start_unix_secs: at,
        end_unix_secs: at + 86400,
    }
}

#[tokio::test]
#[ignore = "requires isolated AETHER_TEST_DATABASE_URL; exercises real transactions"]
async fn live_daily_cost_late_attempts_replay_and_rollback() {
    let (admin, first, second, schema) = fixture().await;
    let result = AssertUnwindSafe(async {
        let usage = SqlxUsageReadRepository::new(first.clone());
        let funds = SqlxSettlementRepository::new(first.clone());
        let peer = SqlxSettlementRepository::new(second.clone());
        let day = 1_800_000_000;
        let q1 = quote("daily-attempt", "a");
        let q2 = quote("daily-attempt", "b");
        usage.upsert(parent("daily-attempt")).await.unwrap();
        for q in [&q1, &q2] {
            funds
                .reserve_request_attempt_funds(q.clone())
                .await
                .unwrap();
            funds
                .mark_request_attempt_funds_dispatched(q.identity())
                .await
                .unwrap();
        }
        let first_facts = facts(&q1, Some(6_000_000), false);
        funds
            .record_request_attempt_funds_outcome(first_facts.clone())
            .await
            .unwrap();
        assert_eq!(
            usage
                .read_daily_actual_cost_units(&window(day))
                .await
                .unwrap()
                .key_units,
            0
        );
        funds
            .close_request_funds_admission(CloseRequestFundsAdmissionInput {
                identity: q1.identity(),
                closed_at_unix_secs: day + 86399,
            })
            .await
            .unwrap();
        let before = usage
            .read_daily_actual_cost_units(&window(day))
            .await
            .unwrap();
        assert_eq!(
            (before.user_units, before.key_units),
            (6_000_000, 6_000_000)
        );

        let mut late = facts(&q2, Some(7_000_000), false);
        late.finalized_at_unix_secs = day + 86410;
        // Force contribution validation to fail after the child financial write.
        // The surrounding transaction must roll back the child, wallet and summary.
        let identity: String = sqlx::query_scalar("SELECT usage_id FROM usage_daily_cost_contributions WHERE request_id='daily-attempt'")
            .fetch_one(&first).await.unwrap();
        let before_balance = balance(&first).await;
        sqlx::query("UPDATE usage_daily_cost_contributions SET usage_id='conflicting-audit' WHERE request_id='daily-attempt'")
            .execute(&first).await.unwrap();
        assert!(funds.record_request_attempt_funds_outcome(late.clone()).await.is_err());
        assert_eq!(balance(&first).await, before_balance);
        assert_eq!(summary(&first, "daily-attempt").await.known_actual_cost_units, 6_000_000);
        let still_unknown: bool = sqlx::query_scalar("SELECT terminal_facts IS NULL FROM request_fund_reservations WHERE reservation_token=$1")
            .bind(&q2.quote.identity.reservation_token).fetch_one(&first).await.unwrap();
        assert!(still_unknown);
        sqlx::query("UPDATE usage_daily_cost_contributions SET usage_id=$1 WHERE request_id='daily-attempt'")
            .bind(identity).execute(&first).await.unwrap();
        let (a, b) = tokio::join!(
            funds.record_request_attempt_funds_outcome(late.clone()),
            peer.record_request_attempt_funds_outcome(late.clone())
        );
        a.unwrap();
        b.unwrap();
        funds
            .record_request_attempt_funds_outcome(first_facts)
            .await
            .unwrap();
        funds
            .close_request_funds_admission(CloseRequestFundsAdmissionInput {
                identity: q2.identity(),
                closed_at_unix_secs: day + 86420,
            })
            .await
            .unwrap();
        for status in ["failed", "cancelled", "completed"] {
            let mut event = parent("daily-attempt");
            event.updated_at_unix_secs += 100;
            event.status = status.into();
            event.finalized_at_unix_secs = Some(day + 86430);
            event.actual_total_cost_usd = Some(999.0);
            usage.upsert(event.clone()).await.unwrap();
            usage.upsert(event).await.unwrap();
        }
        let after = usage
            .read_daily_actual_cost_units(&window(day))
            .await
            .unwrap();
        assert_eq!(
            (after.user_units, after.key_units),
            (13_000_000, 13_000_000)
        );
        assert_eq!(
            usage
                .read_daily_actual_cost_units(&window(day + 86400))
                .await
                .unwrap()
                .key_units,
            0
        );

        assert_eq!(
            usage
                .read_daily_actual_cost_units(&window(day))
                .await
                .unwrap()
                .key_units,
            13_000_000
        );
        sqlx::query("DELETE FROM usage WHERE request_id='daily-attempt'")
            .execute(&first)
            .await
            .unwrap();
        assert_eq!(
            usage
                .read_daily_actual_cost_units(&window(day))
                .await
                .unwrap()
                .key_units,
            13_000_000
        );
        assert!(usage.upsert(parent("daily-attempt")).await.is_err());
        assert!(usage
            .find_by_request_id("daily-attempt")
            .await
            .unwrap()
            .is_none());
    })
    .catch_unwind()
    .await;
    cleanup(admin, first, second, schema).await;
    result.unwrap();
}

#[tokio::test]
#[ignore = "requires isolated AETHER_TEST_DATABASE_URL; exercises real transactions"]
async fn live_daily_cost_legacy_identity_scopes_and_concurrent_parents() {
    let (admin, first, second, schema) = fixture().await;
    let result = AssertUnwindSafe(async {
        let usage = SqlxUsageReadRepository::new(first.clone());
        let other = SqlxUsageReadRepository::new(second.clone());
        let day = 1_800_000_000;
        let mut event = parent("daily-legacy");
        event.status = "completed".into();
        event.finalized_at_unix_secs = Some(day + 1);
        event.actual_total_cost_usd = Some(0.06);
        usage.upsert(event.clone()).await.unwrap();
        let (a, b) = tokio::join!(usage.upsert(event.clone()), other.upsert(event.clone()));
        a.unwrap();
        b.unwrap();
        assert_eq!(
            usage
                .read_daily_actual_cost_units(&window(day))
                .await
                .unwrap()
                .key_units,
            6_000_000
        );
        let mut standalone = parent("daily-standalone");
        standalone.status = "completed".into();
        standalone.finalized_at_unix_secs = Some(day + 2);
        standalone.actual_total_cost_usd = Some(0.07);
        standalone.request_metadata = Some(serde_json::json!({"api_key_is_standalone":true}));
        let mut ordinary = event.clone();
        ordinary.request_id = "daily-second".into();
        let (a, b) = tokio::join!(usage.upsert(standalone), other.upsert(ordinary));
        a.unwrap();
        b.unwrap();
        let counts = usage
            .read_daily_actual_cost_units(&window(day))
            .await
            .unwrap();
        assert_eq!(
            (counts.user_units, counts.key_units),
            (12_000_000, 19_000_000)
        );
        event.updated_at_unix_secs += 10;
        event.finalized_at_unix_secs = Some(day + 86401);
        event.user_id = Some("new-owner".into());
        event.api_key_id = Some("key-b".into());
        event.request_metadata = Some(serde_json::json!({"api_key_is_standalone":true}));
        usage.upsert(event).await.unwrap();
        let counts = usage
            .read_daily_actual_cost_units(&window(day))
            .await
            .unwrap();
        assert_eq!(
            (counts.user_units, counts.key_units),
            (12_000_000, 19_000_000)
        );
        assert_eq!(
            usage
                .read_daily_actual_cost_units(&window(day + 86400))
                .await
                .unwrap()
                .key_units,
            0
        );
    })
    .catch_unwind()
    .await;
    cleanup(admin, first, second, schema).await;
    result.unwrap();
}

#[tokio::test]
#[ignore = "requires isolated AETHER_TEST_DATABASE_URL; exercises executable migration backfill"]
async fn live_daily_cost_backfill_replays_source_and_preserves_frozen_identity() {
    let (admin, first, second, schema) = fixture().await;
    let result = AssertUnwindSafe(async {
        let definition: String = sqlx::query_scalar("SELECT pg_get_functiondef('public.backfill_usage_daily_cost_contributions()'::regprocedure)")
            .fetch_one(&admin).await.unwrap();
        sqlx::raw_sql(&definition.replace("public.", &format!("{schema}.")))
            .execute(&first).await.unwrap();
        let backfill = format!("SELECT {schema}.backfill_usage_daily_cost_contributions()");
        let day = 1_800_000_000;
        let usage = SqlxUsageReadRepository::new(first.clone());
        let funds = SqlxSettlementRepository::new(first.clone());
        let mut legacy = parent("backfill-legacy");
        legacy.status = "completed".into();
        legacy.finalized_at_unix_secs = Some(day + 1);
        legacy.actual_total_cost_usd = Some(0.06);
        usage.upsert(legacy).await.unwrap();
        let q = quote("backfill-attempt", "a");
        usage.upsert(parent("backfill-attempt")).await.unwrap();
        funds.reserve_request_attempt_funds(q.clone()).await.unwrap();
        funds.mark_request_attempt_funds_dispatched(q.identity()).await.unwrap();
        funds.record_request_attempt_funds_outcome(facts(&q, Some(7_000_000), false)).await.unwrap();
        funds.close_request_funds_admission(CloseRequestFundsAdmissionInput {
            identity: q.identity(), closed_at_unix_secs: day + 2,
        }).await.unwrap();
        // Simulate data written before the new ledger existed; neither Redis nor
        // the attempt display float can establish the integer financial truth.
        sqlx::raw_sql("DELETE FROM usage_daily_cost_contributions; UPDATE usage SET actual_total_cost_usd=99 WHERE request_id='backfill-attempt'")
            .execute(&first).await.unwrap();
        let count: i64 = sqlx::query_scalar(&backfill).fetch_one(&first).await.unwrap();
        assert_eq!(count, 2);
        let (a, b) = tokio::join!(sqlx::query_scalar::<_, i64>(&backfill).fetch_one(&first),
            sqlx::query_scalar::<_, i64>(&backfill).fetch_one(&second));
        assert_eq!(a.unwrap(), 2); assert_eq!(b.unwrap(), 2);
        let counts = usage.read_daily_actual_cost_units(&window(day)).await.unwrap();
        assert_eq!((counts.user_units, counts.key_units), (13_000_000, 13_000_000));
        sqlx::raw_sql("UPDATE usage SET user_id='changed',api_key_id='key-b',request_metadata='{\"api_key_is_standalone\":true}',funds_admission_closed_at=to_timestamp(1800086402) WHERE request_id='backfill-attempt'")
            .execute(&first).await.unwrap();
        sqlx::query_scalar::<_, i64>(&backfill).fetch_one(&first).await.unwrap();
        assert_eq!(usage.read_daily_actual_cost_units(&window(day)).await.unwrap(), counts);
        sqlx::query("DELETE FROM usage WHERE request_id='backfill-legacy'").execute(&first).await.unwrap();
        sqlx::query_scalar::<_, i64>(&backfill).fetch_one(&first).await.unwrap();
        assert_eq!(usage.read_daily_actual_cost_units(&window(day)).await.unwrap(), counts);
    }).catch_unwind().await;
    cleanup(admin, first, second, schema).await;
    result.unwrap();
}

#[tokio::test]
#[ignore = "requires isolated AETHER_TEST_DATABASE_URL; exercises batch identity transactions"]
async fn live_daily_cost_pending_batches_freeze_identity_and_rollback_reuse() {
    let (admin, first, second, schema) = fixture().await;
    let result = AssertUnwindSafe(async {
        let usage = SqlxUsageReadRepository::new(first.clone());
        usage
            .upsert_pending_many(vec![parent("batch-a"), parent("batch-b")])
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM usage_daily_cost_contributions")
            .fetch_one(&first)
            .await
            .unwrap();
        assert_eq!(count, 2);
        let mut streaming = parent("batch-a");
        streaming.status = "streaming".into();
        streaming.updated_at_unix_secs += 1;
        usage.upsert_first_byte_many(vec![streaming]).await.unwrap();
        let mut final_event = parent("batch-a");
        final_event.status = "completed".into();
        final_event.actual_total_cost_usd = Some(0.06);
        final_event.updated_at_unix_secs += 10;
        final_event.finalized_at_unix_secs = Some(1_800_000_001);
        final_event.api_key_id = Some("key-b".into());
        final_event.request_metadata = Some(serde_json::json!({"api_key_is_standalone":true}));
        usage.upsert(final_event).await.unwrap();
        let counts = usage
            .read_daily_actual_cost_units(&window(1_800_000_000))
            .await
            .unwrap();
        assert_eq!(
            (counts.user_units, counts.key_units),
            (6_000_000, 6_000_000)
        );
        sqlx::query("DELETE FROM usage WHERE request_id='batch-a'")
            .execute(&first)
            .await
            .unwrap();
        assert!(usage
            .upsert_pending_many(vec![parent("batch-a"), parent("batch-new")])
            .await
            .is_err());
        assert!(usage
            .find_by_request_id("batch-new")
            .await
            .unwrap()
            .is_none());
        assert!(usage.find_by_request_id("batch-a").await.unwrap().is_none());
    })
    .catch_unwind()
    .await;
    cleanup(admin, first, second, schema).await;
    result.unwrap();
}

#[tokio::test]
#[ignore = "requires isolated AETHER_TEST_DATABASE_URL; regression for fractional midnight backfill"]
async fn live_daily_cost_fractional_midnight_backfill_keeps_day_after_revision() {
    let (admin, first, second, schema) = fixture().await;
    let result = AssertUnwindSafe(async {
        let definition: String = sqlx::query_scalar("SELECT pg_get_functiondef('public.backfill_usage_daily_cost_contributions()'::regprocedure)")
            .fetch_one(&admin).await.unwrap();
        sqlx::raw_sql(&definition.replace("public.", &format!("{schema}.")))
            .execute(&first).await.unwrap();
        let day = 1_800_000_000;
        let usage = SqlxUsageReadRepository::new(first.clone());
        let mut legacy = parent("fractional-midnight");
        legacy.status = "completed".into();
        legacy.finalized_at_unix_secs = Some(day + 86399);
        legacy.actual_total_cost_usd = Some(0.06);
        usage.upsert(legacy.clone()).await.unwrap();
        sqlx::raw_sql("UPDATE usage SET finalized_at=to_timestamp(1800086399.9) WHERE request_id='fractional-midnight'; UPDATE usage_settlement_snapshots SET finalized_at=to_timestamp(1800086399.9) WHERE request_id='fractional-midnight'; DELETE FROM usage_daily_cost_contributions WHERE request_id='fractional-midnight'")
            .execute(&first).await.unwrap();
        sqlx::query_scalar::<_, i64>(&format!("SELECT {schema}.backfill_usage_daily_cost_contributions()"))
            .fetch_one(&first).await.unwrap();
        assert_eq!(usage.read_daily_actual_cost_units(&window(day)).await.unwrap().key_units, 6_000_000);
        legacy.updated_at_unix_secs += 100;
        legacy.finalized_at_unix_secs = Some(day + 86410);
        legacy.actual_total_cost_usd = Some(0.07);
        usage.upsert(legacy).await.unwrap();
        assert_eq!(usage.read_daily_actual_cost_units(&window(day)).await.unwrap().key_units, 7_000_000);
        assert_eq!(usage.read_daily_actual_cost_units(&window(day + 86400)).await.unwrap().key_units, 0);
        let exact_anchor: bool = sqlx::query_scalar("SELECT accounting_at=to_timestamp(1800086399.9) FROM usage_daily_cost_contributions WHERE request_id='fractional-midnight'")
            .fetch_one(&first).await.unwrap();
        assert!(exact_anchor);
    }).catch_unwind().await;
    cleanup(admin, first, second, schema).await;
    result.unwrap();
}
