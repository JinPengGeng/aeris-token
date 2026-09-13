use super::*;
use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::driver::postgres::SqlxBillingReadRepository;
use aether_data::repository::{
    auth::{InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeySnapshot},
    candidate_selection::InMemoryMinimalCandidateSelectionReadRepository,
    candidates::InMemoryRequestCandidateRepository,
    provider_catalog::InMemoryProviderCatalogReadRepository,
};
use aether_data_contracts::repository::{
    billing::{BillingReadRepository, UserDailyQuotaAvailabilityRecord},
    candidate_selection::StoredMinimalCandidateSelectionRow,
    provider_catalog::{
        StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
    },
};

#[derive(Clone, Copy, Debug)]
enum Account {
    User,
    Standalone,
    Unlimited,
    Entitlement,
}

struct PublicBilling {
    models: InMemoryBillingReadRepository,
    grants: SqlxBillingReadRepository,
}

#[async_trait::async_trait]
impl BillingReadRepository for PublicBilling {
    async fn find_model_context(
        &self,
        provider: &str,
        key: Option<&str>,
        model: &str,
    ) -> Result<Option<StoredBillingModelContext>, aether_data::DataLayerError> {
        self.models.find_model_context(provider, key, model).await
    }

    async fn find_model_context_by_model_id(
        &self,
        provider: &str,
        key: Option<&str>,
        model: &str,
    ) -> Result<Option<StoredBillingModelContext>, aether_data::DataLayerError> {
        self.models
            .find_model_context_by_model_id(provider, key, model)
            .await
    }

    async fn find_user_daily_quota_availability(
        &self,
        user: &str,
    ) -> Result<Option<UserDailyQuotaAvailabilityRecord>, aether_data::DataLayerError> {
        self.grants.find_user_daily_quota_availability(user).await
    }
}

fn candidate() -> StoredMinimalCandidateSelectionRow {
    StoredMinimalCandidateSelectionRow {
        provider_id: "p-a".into(),
        provider_name: "images".into(),
        provider_type: "openai".into(),
        provider_priority: 10,
        provider_is_active: true,
        endpoint_id: "e-a".into(),
        endpoint_api_format: "openai:image".into(),
        endpoint_api_family: Some("openai".into()),
        endpoint_kind: Some("image".into()),
        endpoint_is_active: true,
        key_id: "pk-a".into(),
        key_name: "image-key".into(),
        key_auth_type: "api_key".into(),
        key_is_active: true,
        key_api_formats: Some(vec!["openai:image".into()]),
        key_allowed_models: None,
        key_capabilities: None,
        key_internal_priority: 5,
        key_global_priority_by_format: Some(json!({"openai:image":1})),
        model_id: "image".into(),
        global_model_id: "image-model".into(),
        global_model_name: "image-model".into(),
        global_model_mappings: None,
        global_model_supports_streaming: Some(true),
        model_provider_model_name: "image".into(),
        model_provider_model_mappings: None,
        model_supports_streaming: Some(true),
        model_is_active: true,
        model_is_available: true,
    }
}

impl Fixture {
    async fn public_state(&self, account: Account, upstream: &str) -> AppState {
        match account {
            Account::User => (),
            Account::Standalone => {
                sqlx::raw_sql("UPDATE api_keys SET is_standalone=true WHERE id='key-a'; UPDATE wallets SET user_id=NULL,api_key_id='key-a' WHERE id='wallet'").execute(&self.pool).await.unwrap();
                sqlx::query("INSERT INTO wallets(id,user_id,balance,gift_balance,status,limit_mode,created_at,updated_at) VALUES('owner-wallet','owner',0.30,0,'active','finite',NOW(),NOW())").execute(&self.pool).await.unwrap();
            }
            Account::Unlimited => {
                sqlx::query(
                    "UPDATE wallets SET balance=0,limit_mode='unlimited' WHERE id='wallet'",
                )
                .execute(&self.pool)
                .await
                .unwrap();
            }
            Account::Entitlement => {
                sqlx::query("DELETE FROM wallets WHERE id='wallet'")
                    .execute(&self.pool)
                    .await
                    .unwrap();
                let grant = json!([{"type":"daily_quota","daily_quota_usd":0.20,"reset_timezone":"UTC","allow_wallet_overage":false}]);
                sqlx::query("INSERT INTO billing_plans(id,title,price_amount,duration_unit,duration_value,entitlements_json,created_at,updated_at) VALUES('plan','plan',1,'day',1,$1,NOW(),NOW())").bind(&grant).execute(&self.pool).await.unwrap();
                sqlx::query("INSERT INTO user_plan_entitlements(id,user_id,plan_id,payment_order_id,starts_at,expires_at,entitlements_snapshot,status,created_at,updated_at) VALUES('grant','owner','plan','order',NOW()-INTERVAL '1 hour',NOW()+INTERVAL '1 hour',$1,'active',NOW(),NOW())").bind(&grant).execute(&self.pool).await.unwrap();
            }
        }
        let auth = StoredAuthApiKeySnapshot::new(
            "owner".into(),
            "owner".into(),
            None,
            "user".into(),
            "local".into(),
            true,
            false,
            None,
            Some(json!(["openai:image"])),
            Some(json!(["image-model"])),
            "key-a".into(),
            Some("images".into()),
            true,
            false,
            matches!(account, Account::Standalone),
            None,
            None,
            None,
            None,
            Some(json!(["openai:image"])),
            Some(json!(["image-model"])),
        )
        .unwrap();
        let auth = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed([(
            Some(format!("{:x}", Sha256::digest(b"sk-public-image-fixture"))),
            auth,
        )]));
        let provider = StoredProviderCatalogProvider::new(
            "p-a".into(),
            "images".into(),
            None,
            "openai".into(),
        )
        .unwrap()
        .with_transport_fields(
            true,
            false,
            false,
            None,
            Some(2),
            None,
            Some(10.0),
            None,
            None,
        );
        let base_url = upstream
            .trim_end_matches("/v1/images/generations")
            .to_string();
        let endpoint = StoredProviderCatalogEndpoint::new(
            "e-a".into(),
            "p-a".into(),
            "openai:image".into(),
            Some("openai".into()),
            Some("image".into()),
            true,
        )
        .unwrap()
        .with_transport_fields(base_url, None, None, Some(2), None, None, None, None)
        .unwrap();
        let key = StoredProviderCatalogKey::new(
            "pk-a".into(),
            "p-a".into(),
            "image-key".into(),
            "api_key".into(),
            None,
            true,
        )
        .unwrap()
        .with_transport_fields(
            Some(json!(["openai:image"])),
            encrypt_python_fernet_plaintext(
                DEVELOPMENT_ENCRYPTION_KEY,
                "sk-public-upstream-fixture",
            )
            .unwrap(),
            None,
            None,
            Some(json!({"openai:image":1})),
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let data = GatewayDataState::with_auth_candidate_selection_provider_catalog_request_candidates_usage_billing_and_wallet_for_tests(
            auth,
            Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed([candidate()])),
            Arc::new(InMemoryProviderCatalogReadRepository::seed(vec![provider],vec![endpoint],vec![key])),
            Arc::new(InMemoryRequestCandidateRepository::default()),
            Arc::new(SqlxUsageReadRepository::new(self.pool.clone())),
            Arc::new(PublicBilling { models: InMemoryBillingReadRepository::seed([pricing("a")]), grants: SqlxBillingReadRepository::new(self.pool.clone()) }),
            Arc::new(SqlxWalletRepository::new(self.pool.clone())),
            DEVELOPMENT_ENCRYPTION_KEY,
        ).with_settlement_writer_for_tests(Arc::new(SqlxSettlementRepository::new(self.pool.clone())));
        AppState::new()
            .unwrap()
            .with_data_state_for_tests(data)
            .with_usage_runtime_for_tests(UsageRuntimeConfig {
                enabled: true,
                queue_terminal_events: false,
                ..UsageRuntimeConfig::default()
            })
    }
}

async fn public_server(state: AppState) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            crate::build_router_with_state(state)
                .into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (url, task)
}

fn request_body() -> Value {
    json!({"model":"image-model","prompt":"test","n":8,"size":"1024x1024","quality":"high","output_format":"png"})
}

async fn public_request(gateway: &str, request: &str, body: &Value) -> (http::StatusCode, Value) {
    let response = reqwest::Client::new()
        .post(format!("{gateway}/v1/images/generations"))
        .bearer_auth("sk-public-image-fixture")
        .header(crate::constants::TRACE_ID_HEADER, request)
        .json(body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let text = response.text().await.unwrap();
    let body =
        serde_json::from_str(&text).unwrap_or_else(|_| panic!("invalid response {status}: {text}"));
    (status, body)
}

#[tokio::test]
#[ignore = "requires isolated AETHER_TEST_DATABASE_URL and actual public Gateway HTTP"]
async fn live_public_images_fund_user_standalone_unlimited_and_no_wallet_entitlement() {
    for account in [
        Account::User,
        Account::Standalone,
        Account::Unlimited,
        Account::Entitlement,
    ] {
        let fixture = Fixture::new(0.20).await;
        let (upstream_url, calls, upstream_server) = upstream(vec![(200, image(6))]).await;
        let state = fixture.public_state(account, &upstream_url).await;
        let (gateway, gateway_server) = public_server(state.clone()).await;
        let (status, body) = public_request(&gateway, "public-funded-image", &request_body()).await;
        assert_eq!(status, http::StatusCode::OK, "{account:?}: {body}");
        assert_eq!(
            body["data"].as_array().map(Vec::len),
            Some(6),
            "{account:?}: {body}"
        );
        let summary = fixture.summary("public-funded-image").await;
        assert!(summary.admission_closed, "{account:?}: {summary:?}");
        assert_eq!(summary.held_cost_units, 0);
        assert_eq!(summary.known_actual_cost_units, 6_000_000);
        let parent: (String,String,i64) = sqlx::query_as("SELECT status,billing_status,(actual_total_cost_usd*100000000)::bigint FROM usage WHERE request_id='public-funded-image'").fetch_one(&fixture.pool).await.unwrap();
        assert_eq!(parent, ("completed".into(), "settled".into(), 6_000_000));
        match account {
            Account::User => {
                assert!((fixture.balance().await - 0.14).abs() < 0.00000001)
            }
            Account::Standalone => {
                assert!((fixture.balance().await - 0.14).abs() < 0.00000001);
                let owner_balance: f64 = sqlx::query_scalar(
                    "SELECT balance::double precision FROM wallets WHERE id='owner-wallet'",
                )
                .fetch_one(&fixture.pool)
                .await
                .unwrap();
                assert!((owner_balance - 0.30).abs() < 0.00000001);
                let ownership: (Option<String>, bool) = sqlx::query_as("SELECT user_id,(request_metadata->>'api_key_is_standalone')::boolean FROM usage WHERE request_id='public-funded-image'").fetch_one(&fixture.pool).await.unwrap();
                assert_eq!(ownership, (Some("owner".into()), true));
            }
            Account::Unlimited => {
                // Existing unlimited-wallet accounting leaves cash unchanged
                // and records consumption against the postpaid allocation.
                assert_eq!(fixture.balance().await, 0.0);
                let consumed: i64 = sqlx::query_scalar(
                    "SELECT (total_consumed*100000000)::bigint FROM wallets WHERE id='wallet'",
                )
                .fetch_one(&fixture.pool)
                .await
                .unwrap();
                assert_eq!(consumed, 6_000_000);
                let collected: i64 = sqlx::query_scalar("SELECT SUM(collected_cost_units)::bigint FROM request_fund_allocations WHERE source_kind='postpaid'").fetch_one(&fixture.pool).await.unwrap();
                assert_eq!(collected, 6_000_000);
            }
            Account::Entitlement => {
                let debit:i64 = sqlx::query_scalar("SELECT (SUM(amount_usd)*100000000)::bigint FROM entitlement_usage_ledgers WHERE request_id='public-funded-image'").fetch_one(&fixture.pool).await.unwrap();
                assert_eq!(debit, 6_000_000);
            }
        }
        assert_upstream_calls(&upstream_url, &calls, 1).await;
        gateway_server.abort();
        upstream_server.abort();
        fixture.close(state).await;
    }
}

#[tokio::test]
#[ignore = "requires isolated AETHER_TEST_DATABASE_URL and actual public Gateway HTTP"]
async fn live_public_images_reject_unbounded_and_stream_before_every_account_shortcut() {
    for account in [
        Account::User,
        Account::Standalone,
        Account::Unlimited,
        Account::Entitlement,
    ] {
        let fixture = Fixture::new(0.20).await;
        let (upstream_url, calls, upstream_server) = upstream(vec![]).await;
        let state = fixture.public_state(account, &upstream_url).await;
        let (gateway, gateway_server) = public_server(state.clone()).await;
        for (request, field, value) in [
            ("unbounded", "size", json!("auto")),
            ("streamed", "stream", json!(true)),
        ] {
            let mut body = request_body();
            body[field] = value;
            let (status, body) = public_request(&gateway, request, &body).await;
            // Sync heartbeat may already have sent 200 before delivering the
            // structured admission error in its JSON body.
            assert!(
                status == http::StatusCode::UNPROCESSABLE_ENTITY || status == http::StatusCode::OK,
                "{account:?}: {status} {body}"
            );
            assert!(body.get("error").is_some(), "{account:?}: {body}");
        }
        let reservations: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM request_fund_reservations")
                .fetch_one(&fixture.pool)
                .await
                .unwrap();
        assert_eq!(reservations, 0);
        assert_upstream_calls(&upstream_url, &calls, 0).await;
        gateway_server.abort();
        upstream_server.abort();
        fixture.close(state).await;
    }
}

#[tokio::test]
#[ignore = "requires isolated AETHER_TEST_DATABASE_URL and actual public Gateway HTTP"]
async fn live_public_image_retry_reserves_each_send_and_retains_unknown_hold() {
    for (balance, expected_calls) in [(0.10, 1_usize), (0.20, 2_usize)] {
        let fixture = Fixture::new(balance).await;
        let (upstream_url, calls, upstream_server) = upstream(vec![
            (500, json!({"error":{"message":"temporarily unavailable"}})),
            (200, image(6)),
        ])
        .await;
        let state = fixture.public_state(Account::User, &upstream_url).await;
        let (gateway, gateway_server) = public_server(state.clone()).await;
        let (_, body) = public_request(&gateway, "public-retry-image", &request_body()).await;
        if expected_calls == 2 {
            assert_eq!(body["data"].as_array().map(Vec::len), Some(6), "{body}");
        } else {
            assert!(body.get("error").is_some(), "{body}");
        }
        assert_upstream_calls(&upstream_url, &calls, expected_calls).await;
        let summary = fixture.summary("public-retry-image").await;
        assert!(summary.admission_closed);
        assert_eq!(summary.unknown_attempts, 1);
        assert_eq!(summary.held_cost_units, 8_000_000);
        assert_eq!(
            summary.known_actual_cost_units,
            if expected_calls == 2 { 6_000_000 } else { 0 }
        );
        let identities: (i64,i64,i64) = sqlx::query_as("SELECT COUNT(*),COUNT(DISTINCT attempt_id),COUNT(DISTINCT reservation_token) FROM request_fund_reservations WHERE request_id='public-retry-image'").fetch_one(&fixture.pool).await.unwrap();
        let expected = i64::try_from(expected_calls).unwrap();
        assert_eq!(identities, (expected, expected, expected));
        let parent_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM usage WHERE request_id='public-retry-image'")
                .fetch_one(&fixture.pool)
                .await
                .unwrap();
        assert_eq!(parent_count, 1);
        gateway_server.abort();
        upstream_server.abort();
        fixture.close(state).await;
    }
}
