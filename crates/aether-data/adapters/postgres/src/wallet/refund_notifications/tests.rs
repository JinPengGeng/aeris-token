use crate::settlement::funding::tests::fixture;
use crate::SqlxWalletRepository;
use aether_data_contracts::repository::wallet::*;
use futures_util::FutureExt;
use sqlx::PgPool;
use std::panic::AssertUnwindSafe;

async fn setup(pool: &PgPool) {
    sqlx::query("CREATE TABLE IF NOT EXISTS refund_status_notifications (LIKE public.refund_status_notifications INCLUDING ALL)").execute(pool).await.unwrap();
}
async fn refund(pool: &PgPool, id: &str, amount: f64) {
    sqlx::query("INSERT INTO refund_requests(id,refund_no,wallet_id,user_id,amount_usd,status,source_type,refund_mode,created_at,updated_at) VALUES($1,$1,'wallet','owner',$2,'approved','wallet','offline_payout',now(),now())")
        .bind(id).bind(amount).execute(pool).await.unwrap();
}
fn process(id: &str) -> ProcessAdminWalletRefundInput {
    ProcessAdminWalletRefundInput {
        wallet_id: "wallet".into(),
        refund_id: id.into(),
        operator_id: None,
    }
}
fn complete(id: &str) -> CompleteAdminWalletRefundInput {
    CompleteAdminWalletRefundInput {
        wallet_id: "wallet".into(),
        refund_id: id.into(),
        gateway_refund_id: None,
        payout_reference: None,
        payout_proof: None,
    }
}
fn fail(id: &str) -> FailAdminWalletRefundInput {
    FailAdminWalletRefundInput {
        wallet_id: "wallet".into(),
        refund_id: id.into(),
        reason: "internal SQL error Authorization: Bearer synthetic-secret-token".into(),
        operator_id: None,
    }
}
async fn financial(pool: &PgPool) -> (i64, i64, i64) {
    sqlx::query_as("SELECT ROUND(balance*100000000)::bigint,ROUND(total_refunded*100000000)::bigint,(SELECT COUNT(*) FROM wallet_transactions) FROM wallets WHERE id='wallet'")
        .fetch_one(pool).await.unwrap()
}
async fn due(pool: &PgPool, id: &str) {
    sqlx::query("UPDATE refund_status_notifications SET next_attempt_at=now()-interval '1 second' WHERE id=$1").bind(id).execute(pool).await.unwrap();
}
async fn ack(
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

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; atomic terminal refund and outbox rollback"]
async fn live_refund_outbox_all_terminal_branches_roll_back_when_enqueue_fails() {
    let (admin, pool, other, schema) = fixture().await;
    let result=AssertUnwindSafe(async {
        setup(&pool).await;
        let repo=SqlxWalletRepository::new(pool.clone());
        for (id,amount) in [("approved-fail",0.01),("success",0.03),("processing-fail",0.02)] { refund(&pool,id,amount).await; }
        for id in ["success","processing-fail"] { assert!(matches!(repo.process_admin_wallet_refund(process(id)).await.unwrap(),WalletMutationOutcome::Applied(_))); }
        // Provider success evidence is a separate already-committed fact.
        assert!(matches!(repo.update_admin_wallet_refund_gateway(UpdateAdminWalletRefundGatewayInput{wallet_id:"wallet".into(),refund_id:"success".into(),gateway_refund_id:"synthetic-provider-refund".into(),payout_proof:Some(serde_json::json!({"status":"success"}))}).await.unwrap(),WalletMutationOutcome::Applied(_)));
        let before=financial(&pool).await;
        sqlx::raw_sql("CREATE FUNCTION reject_refund_event() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic enqueue failure'; END $$; CREATE TRIGGER reject_refund_event BEFORE INSERT ON refund_status_notifications FOR EACH ROW EXECUTE FUNCTION reject_refund_event()")
            .execute(&pool).await.unwrap();
        assert!(repo.fail_admin_wallet_refund(fail("approved-fail")).await.is_err());
        assert!(repo.complete_admin_wallet_refund(complete("success")).await.is_err());
        assert!(repo.fail_admin_wallet_refund(fail("processing-fail")).await.is_err());
        assert_eq!(financial(&pool).await,before);
        let states:Vec<(String,String)>=sqlx::query_as("SELECT id,status FROM refund_requests ORDER BY id").fetch_all(&pool).await.unwrap();
        assert_eq!(states,vec![("approved-fail".into(),"approved".into()),("processing-fail".into(),"processing".into()),("success".into(),"processing".into())]);
        let evidence:(String,serde_json::Value)=sqlx::query_as("SELECT gateway_refund_id,payout_proof FROM refund_requests WHERE id='success'").fetch_one(&pool).await.unwrap();
        assert_eq!(evidence,("synthetic-provider-refund".into(),serde_json::json!({"status":"success"})));
        sqlx::query("DROP TRIGGER reject_refund_event ON refund_status_notifications").execute(&pool).await.unwrap();
        assert!(matches!(repo.fail_admin_wallet_refund(fail("approved-fail")).await.unwrap(),WalletMutationOutcome::Applied(_)));
        assert!(matches!(repo.complete_admin_wallet_refund(complete("success")).await.unwrap(),WalletMutationOutcome::Applied(_)));
        assert!(matches!(repo.fail_admin_wallet_refund(fail("processing-fail")).await.unwrap(),WalletMutationOutcome::Applied(_)));
        assert_eq!(financial(&pool).await,(7_000_000,3_000_000,3));
        let events:Vec<(String,String)>=sqlx::query_as("SELECT id,terminal_status FROM refund_status_notifications ORDER BY id").fetch_all(&pool).await.unwrap();
        assert_eq!(events,vec![("refund:approved-fail:failed".into(),"failed".into()),("refund:processing-fail:failed".into(),"failed".into()),("refund:success:succeeded".into(),"succeeded".into())]);
        let restarted=SqlxWalletRepository::new(other.clone());
        assert_eq!(restarted.claim_refund_status_notifications(10).await.unwrap().len(),3);
        assert_eq!(financial(&pool).await,(7_000_000,3_000_000,3));
    }).catch_unwind().await;
    pool.close().await;
    other.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic)
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; concurrent terminal replay never duplicates money or logical event"]
async fn live_refund_outbox_concurrent_replay_and_restart_preserve_single_event() {
    let (admin, pool, other, schema) = fixture().await;
    let result = AssertUnwindSafe(async {
        setup(&pool).await;
        let a = SqlxWalletRepository::new(pool.clone());
        let b = SqlxWalletRepository::new(other.clone());
        refund(&pool, "complete-race", 0.03).await;
        refund(&pool, "fail-race", 0.02).await;
        for id in ["complete-race", "fail-race"] {
            a.process_admin_wallet_refund(process(id)).await.unwrap();
        }
        let (left, right) = tokio::join!(
            a.complete_admin_wallet_refund(complete("complete-race")),
            b.complete_admin_wallet_refund(complete("complete-race"))
        );
        assert!(matches!(left.unwrap(), WalletMutationOutcome::Applied(_)));
        assert!(matches!(right.unwrap(), WalletMutationOutcome::Applied(_)));
        let (left, right) = tokio::join!(
            a.fail_admin_wallet_refund(fail("fail-race")),
            b.fail_admin_wallet_refund(fail("fail-race"))
        );
        let results = [left.unwrap(), right.unwrap()];
        assert_eq!(
            results
                .iter()
                .filter(|r| matches!(r, WalletMutationOutcome::Applied(_)))
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|r| matches!(r, WalletMutationOutcome::Invalid(_)))
                .count(),
            1
        );
        assert_eq!(financial(&pool).await, (7_000_000, 3_000_000, 3));
        // Simulate an unobserved successful response by replaying with a new repository.
        let restarted = SqlxWalletRepository::new(other.clone());
        restarted
            .complete_admin_wallet_refund(complete("complete-race"))
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM refund_status_notifications")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 2);
        let (left, right) = tokio::join!(
            a.claim_refund_status_notifications(10),
            restarted.claim_refund_status_notifications(10)
        );
        let mut events = left.unwrap();
        events.extend(right.unwrap());
        assert_eq!(events.len(), 2);
        assert_ne!(events[0].id, events[1].id);
        for event in events {
            assert!(ack(&restarted, &event, RefundNotificationOutcome::Delivered).await);
        }
        // Importing historical terminal records must not synthesize delivery.
        refund(&pool, "historical-import", 0.01).await;
        sqlx::query("UPDATE refund_requests SET status='succeeded' WHERE id='historical-import'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            restarted
                .complete_admin_wallet_refund(complete("historical-import"))
                .await
                .unwrap(),
            WalletMutationOutcome::Applied(_)
        ));
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM refund_status_notifications")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 2);
        assert_eq!(financial(&pool).await, (7_000_000, 3_000_000, 3));
    })
    .catch_unwind()
    .await;
    pool.close().await;
    other.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic)
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; lease fencing and skipped delivery retry budget"]
async fn live_refund_outbox_fences_expired_leases_and_counts_only_delivery_failures() {
    let (admin, pool, other, schema) = fixture().await;
    let result=AssertUnwindSafe(async {
        setup(&pool).await;let a=SqlxWalletRepository::new(pool.clone());let b=SqlxWalletRepository::new(other.clone());
        refund(&pool,"lease",0.01).await;a.fail_admin_wallet_refund(fail("lease")).await.unwrap();
        let first=a.claim_refund_status_notifications(1).await.unwrap().remove(0);
        assert_eq!(first.failure_reason.as_deref(),Some("退款未完成，请在账户中查看退款详情或联系管理员。"));
        let original_reason: String = sqlx::query_scalar("SELECT failure_reason FROM refund_requests WHERE id='lease'").fetch_one(&pool).await.unwrap();
        assert!(original_reason.contains("synthetic-secret-token"), "internal source evidence stays intact while delivery snapshot is fixed wording");
        assert!(b.claim_refund_status_notifications(1).await.unwrap().is_empty());
        sqlx::query("UPDATE refund_status_notifications SET lease_until=clock_timestamp()+interval '1 second' WHERE id=$1").bind(&first.id).execute(&pool).await.unwrap();
        let mut old_transaction=pool.begin().await.unwrap();
        let before_expiry:bool=sqlx::query_scalar("SELECT now()<lease_until FROM refund_status_notifications WHERE id=$1").bind(&first.id).fetch_one(&mut *old_transaction).await.unwrap();
        assert!(before_expiry,"fixture transaction must begin before lease expires");
        sqlx::query("SELECT pg_sleep(1.1)").execute(&mut *old_transaction).await.unwrap();
        assert!(!super::complete(&mut old_transaction,CompleteRefundStatusNotificationInput{id:first.id.clone(),lease_token:first.lease_token,outcome:RefundNotificationOutcome::Delivered}).await.unwrap(), "a transaction opened before expiry must not ACK after real expiry");
        old_transaction.commit().await.unwrap();
        assert!(!ack(&a,&first,RefundNotificationOutcome::Delivered).await);
        let mut event=b.claim_refund_status_notifications(1).await.unwrap().remove(0);assert!(event.lease_token>first.lease_token);
        assert!(!ack(&a,&first,RefundNotificationOutcome::Retry).await);
        for _ in 0..8 {assert!(ack(&a,&event,RefundNotificationOutcome::Skipped).await);due(&pool,&event.id).await;event=b.claim_refund_status_notifications(1).await.unwrap().remove(0);}
        let attempts:i32=sqlx::query_scalar("SELECT attempts FROM refund_status_notifications WHERE id=$1").bind(&event.id).fetch_one(&pool).await.unwrap();assert_eq!(attempts,0);
        for (attempt,delay) in [60,300,1800,7200,21600,86400,86400].into_iter().enumerate() {
            assert!(ack(&a,&event,RefundNotificationOutcome::Retry).await);
            let (state,count,seconds):(String,i32,i64)=sqlx::query_as("SELECT state,attempts,EXTRACT(EPOCH FROM next_attempt_at-clock_timestamp())::bigint FROM refund_status_notifications WHERE id=$1").bind(&event.id).fetch_one(&pool).await.unwrap();
            assert_eq!(count,attempt as i32+1);assert_eq!(state,if attempt==6 {"manual_review"} else {"retry"});assert!((delay-3..=delay+1).contains(&seconds));
            due(&pool,&event.id).await;
            if attempt<6 {event=b.claim_refund_status_notifications(1).await.unwrap().remove(0);} else {assert!(b.claim_refund_status_notifications(1).await.unwrap().is_empty());}
        }
        assert_eq!(financial(&pool).await,(10_000_000,0,0));
    }).catch_unwind().await;
    pool.close().await;
    other.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic)
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; recipient and terminal state integrity"]
async fn live_refund_outbox_quarantines_changed_owner_and_terminal_state() {
    let (admin, pool, other, schema) = fixture().await;
    let result = AssertUnwindSafe(async {
        setup(&pool).await;
        let repo = SqlxWalletRepository::new(pool.clone());
        for (id, mutation) in [
            (
                "wrong-owner",
                "UPDATE refund_requests SET user_id='another-owner' WHERE id='wrong-owner'",
            ),
            (
                "wrong-state",
                "UPDATE refund_requests SET status='processing' WHERE id='wrong-state'",
            ),
            (
                "wrong-wallet",
                "UPDATE refund_requests SET wallet_id='another-wallet' WHERE id='wrong-wallet'",
            ),
            (
                "standalone",
                "UPDATE wallets SET user_id=NULL,api_key_id='key-a' WHERE id='wallet'",
            ),
        ] {
            refund(&pool, id, 0.01).await;
            assert!(matches!(
                repo.fail_admin_wallet_refund(fail(id)).await.unwrap(),
                WalletMutationOutcome::Applied(_)
            ));
            // Shared schema fixtures omit FKs so individual ownership corruption
            // can be exercised without also changing another guard condition.
            sqlx::query(mutation).execute(&pool).await.unwrap();
            assert!(repo
                .claim_refund_status_notifications(10)
                .await
                .unwrap()
                .is_empty());
        }
        let states: Vec<(String, i32, String)> = sqlx::query_as(
            "SELECT state,attempts,error_code FROM refund_status_notifications ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            states,
            vec![
                (
                    "manual_review".into(),
                    0,
                    "refund_owner_or_state_changed".into()
                );
                4
            ]
        );
        assert_eq!(financial(&pool).await, (10_000_000, 0, 0));
    })
    .catch_unwind()
    .await;
    pool.close().await;
    other.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
