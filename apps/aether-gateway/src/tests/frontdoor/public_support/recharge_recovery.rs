use super::{
    build_router_with_state, build_test_auth_token, json, sample_auth_session, sample_auth_user,
    start_server, AppState, Arc, GatewayDataState, InMemoryUserReadRepository, Mutex, StatusCode,
    Utc, TRUSTED_ADMIN_SESSION_ID_HEADER, TRUSTED_ADMIN_USER_ID_HEADER,
    TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use aether_data_contracts::repository::settlement::{
    ReconcileUsagePolicyCostInput, ReleaseUsagePolicyRequestAdmissionInput,
    ReserveUsagePolicyCostInput, ReserveUsagePolicyCostOutcome, ReserveUsagePolicyRequestInput,
    ReserveUsagePolicyRequestOutcome, SettlementWriteRepository, StoredRechargeRecoveryJob,
    StoredUsagePolicyCostReservation, StoredUsagePolicyRequestAdmission, StoredUsageSettlement,
    UsageSettlementInput,
};
use aether_data_contracts::DataLayerError;
use serde_json::Value;
use std::collections::BTreeSet;
use std::time::Duration;

const PATH: &str = "/api/wallet/recharge-recoveries";
const PRIVATE_SOURCE: &str = "source-transaction-private-fixture";
const PRIVATE_ERROR: &str = "repository-private-lease-token-and-source-transaction";

#[derive(Clone, Copy)]
enum ReadMode {
    OwnerFiltered,
    IncorrectOwner,
    Fail,
}

struct RecoveryRepository {
    supported: bool,
    mode: ReadMode,
    rows: Vec<StoredRechargeRecoveryJob>,
    calls: Mutex<Vec<(String, usize)>>,
}

impl RecoveryRepository {
    fn new(mode: ReadMode, rows: Vec<StoredRechargeRecoveryJob>) -> Self {
        Self {
            supported: true,
            mode,
            rows,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<(String, usize)> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl SettlementWriteRepository for RecoveryRepository {
    fn supports_recharge_recovery(&self) -> bool {
        self.supported
    }

    async fn list_recharge_recovery_jobs_for_user(
        &self,
        user_id: &str,
        limit: usize,
    ) -> Result<Vec<StoredRechargeRecoveryJob>, DataLayerError> {
        self.calls.lock().unwrap().push((user_id.into(), limit));
        match self.mode {
            ReadMode::OwnerFiltered => Ok(self
                .rows
                .iter()
                .filter(|row| row.user_id.as_deref() == Some(user_id))
                .take(limit)
                .cloned()
                .collect()),
            ReadMode::IncorrectOwner => Ok(self.rows.clone()),
            ReadMode::Fail => Err(DataLayerError::InvalidInput(PRIVATE_ERROR.into())),
        }
    }

    async fn reserve_usage_policy_request(
        &self,
        _: ReserveUsagePolicyRequestInput,
    ) -> Result<ReserveUsagePolicyRequestOutcome, DataLayerError> {
        panic!("GET recovery history must not reserve requests")
    }

    async fn release_usage_policy_request_admission(
        &self,
        _: ReleaseUsagePolicyRequestAdmissionInput,
    ) -> Result<Option<StoredUsagePolicyRequestAdmission>, DataLayerError> {
        panic!("GET recovery history must not release request admissions")
    }

    async fn reserve_usage_policy_cost(
        &self,
        _: ReserveUsagePolicyCostInput,
    ) -> Result<ReserveUsagePolicyCostOutcome, DataLayerError> {
        panic!("GET recovery history must not reserve cost")
    }

    async fn reconcile_usage_policy_cost(
        &self,
        _: ReconcileUsagePolicyCostInput,
    ) -> Result<Option<StoredUsagePolicyCostReservation>, DataLayerError> {
        panic!("GET recovery history must not reconcile cost")
    }

    async fn settle_usage(
        &self,
        _: UsageSettlementInput,
    ) -> Result<Option<StoredUsageSettlement>, DataLayerError> {
        panic!("GET recovery history must not settle usage")
    }
}

fn job(id: &str, owner: Option<&str>) -> StoredRechargeRecoveryJob {
    StoredRechargeRecoveryJob {
        id: id.into(),
        payment_order_id: format!("order-{id}"),
        source_transaction_id: PRIVATE_SOURCE.into(),
        wallet_id: format!("wallet-{id}"),
        user_id: owner.map(str::to_string),
        state: "completed".into(),
        // Include one-unit precision close to the supported integer ceiling.
        principal_cost_units: 4_503_599_627_370_495,
        collected_cost_units: 4_503_599_600_000_001,
        outstanding_cost_units: 27_370_494,
        available_recharge_cost_units: 1,
        retry_count: 2,
        next_attempt_at_unix_secs: None,
        error_code: None,
        created_at_unix_secs: 1_790_000_001,
        updated_at_unix_secs: 1_790_000_002,
    }
}

struct Harness {
    url: String,
    client: reqwest::Client,
    tokens: Vec<String>,
    task: tokio::task::JoinHandle<()>,
}

impl Harness {
    async fn start(repository: Option<Arc<RecoveryRepository>>) -> Self {
        let now = Utc::now();
        let mut users = Vec::new();
        let mut sessions = Vec::new();
        let mut tokens = Vec::new();
        for index in 1..=2 {
            let mut user = sample_auth_user(now);
            user.id = format!("user-auth-{index}");
            user.username = format!("recovery-user-{index}");
            user.email = Some(format!("recovery-user-{index}@example.com"));
            let session_id = format!("session-recovery-{index}");
            let device_id = format!("device-recovery-{index}");
            tokens.push(build_test_auth_token(
                "access",
                serde_json::Map::from_iter([
                    ("user_id".into(), json!(user.id)),
                    ("role".into(), json!(user.role)),
                    (
                        "created_at".into(),
                        json!(user.created_at.map(|value| value.to_rfc3339())),
                    ),
                    ("session_id".into(), json!(session_id)),
                ]),
                now + chrono::Duration::hours(1),
            ));
            sessions.push(sample_auth_session(
                &user.id,
                &session_id,
                &device_id,
                "fixture-refresh-token",
                now,
            ));
            users.push(user);
        }
        let mut data = GatewayDataState::with_user_reader_for_tests(Arc::new(
            InMemoryUserReadRepository::seed_auth_users(users),
        ));
        if let Some(repository) = repository {
            data = data.with_recharge_recovery_repository_for_tests(repository);
        }
        let state = AppState::new()
            .unwrap()
            .with_data_state_for_tests(data)
            .with_auth_sessions_for_tests(sessions);
        let (url, task) = start_server(build_router_with_state(state)).await;
        Self {
            url,
            task,
            tokens,
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap(),
        }
    }

    fn get(&self, index: usize, query: &str) -> reqwest::RequestBuilder {
        self.authorize(self.client.get(format!("{}{PATH}{query}", self.url)), index)
    }

    fn authorize(&self, request: reqwest::RequestBuilder, index: usize) -> reqwest::RequestBuilder {
        request
            .bearer_auth(&self.tokens[index - 1])
            .header("x-client-device-id", format!("device-recovery-{index}"))
            .header("user-agent", "AetherTest/1.0")
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn response(request: reqwest::RequestBuilder) -> (StatusCode, Value) {
    let response = request
        .send()
        .await
        .expect("loopback request should finish");
    let status = response.status();
    let bytes = response.bytes().await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or_else(|_| {
        panic!(
            "expected JSON response; status={status}, bytes={}",
            bytes.len()
        )
    });
    (status, body)
}

fn assert_no_private_fields(body: &Value) {
    let serialized = body.to_string();
    for private in [
        "source_transaction_id",
        "user_id",
        "lease_token",
        "notification",
        PRIVATE_SOURCE,
        PRIVATE_ERROR,
    ] {
        assert!(
            !serialized.contains(private),
            "public response exposed an internal field"
        );
    }
}

#[tokio::test]
async fn gateway_recharge_recoveries_rejects_missing_or_forged_identity_before_repository_access() {
    let repository = Arc::new(RecoveryRepository::new(
        ReadMode::OwnerFiltered,
        vec![job("one", Some("user-auth-1"))],
    ));
    let gateway = Harness::start(Some(repository.clone())).await;
    let endpoint = format!("{}{PATH}?user_id=user-auth-1", gateway.url);
    for request in [
        gateway.client.get(&endpoint),
        gateway
            .client
            .get(&endpoint)
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "user-auth-1")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-recovery-1")
            .header(crate::constants::GATEWAY_HEADER, "rust-phase3b"),
        gateway
            .client
            .get(&endpoint)
            .bearer_auth("forged-user-identity")
            .header("x-client-device-id", "device-recovery-1"),
        gateway
            .client
            .get(&endpoint)
            .bearer_auth(&gateway.tokens[0])
            .header("x-client-device-id", "device-recovery-2")
            .header("user-agent", "AetherTest/1.0"),
    ] {
        let (status, body) = response(request).await;
        assert!(matches!(
            status,
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ));
        assert!(body.get("items").is_none());
        assert_no_private_fields(&body);
    }
    assert!(repository.calls().is_empty());
}

#[tokio::test]
async fn gateway_recharge_recoveries_scopes_owner_and_preserves_public_integer_amounts() {
    let repository = Arc::new(RecoveryRepository::new(
        ReadMode::OwnerFiltered,
        vec![
            job("one", Some("user-auth-1")),
            job("two", Some("user-auth-2")),
        ],
    ));
    let gateway = Harness::start(Some(repository.clone())).await;
    let (status, body) = response(
        gateway
            .get(1, "?limit=1&user_id=user-auth-2&wallet_id=wallet-two")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "user-auth-2")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-recovery-2"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["limit"], 1);
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    let item = &body["items"][0];
    assert_eq!(item["id"], "one");
    assert_eq!(
        item["principal_cost_units"].as_u64(),
        Some(4_503_599_627_370_495)
    );
    assert_eq!(
        item["collected_cost_units"].as_u64(),
        Some(4_503_599_600_000_001)
    );
    assert_eq!(item["outstanding_cost_units"].as_u64(), Some(27_370_494));
    assert_eq!(item["available_recharge_cost_units"].as_u64(), Some(1));
    let fields: BTreeSet<&str> = item
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        fields,
        BTreeSet::from([
            "id",
            "payment_order_id",
            "wallet_id",
            "state",
            "principal_cost_units",
            "collected_cost_units",
            "outstanding_cost_units",
            "available_recharge_cost_units",
            "retry_count",
            "next_attempt_at_unix_secs",
            "error_code",
            "created_at_unix_secs",
            "updated_at_unix_secs",
        ])
    );
    assert_no_private_fields(&body);
    let (status, other) = response(gateway.get(2, "")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(other["limit"], 50);
    assert_eq!(other["items"].as_array().unwrap().len(), 1);
    assert_eq!(other["items"][0]["id"], "two");
    assert_no_private_fields(&other);
    assert_eq!(
        repository.calls(),
        vec![("user-auth-1".into(), 1), ("user-auth-2".into(), 50)]
    );

    let denied = gateway
        .authorize(gateway.client.post(format!("{}{PATH}", gateway.url)), 1)
        .json(&json!({"user_id":"user-auth-2"}))
        .send()
        .await
        .unwrap();
    assert!(matches!(
        denied.status(),
        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED | StatusCode::NOT_IMPLEMENTED
    ));
    assert_eq!(
        repository.calls().len(),
        2,
        "POST must not execute the GET repository query"
    );
}

#[tokio::test]
async fn gateway_recharge_recoveries_fails_closed_for_foreign_missing_or_empty_owner_rows() {
    for owner in [Some("user-auth-2"), None, Some("")] {
        let repository = Arc::new(RecoveryRepository::new(
            ReadMode::IncorrectOwner,
            vec![
                job("legitimate-row", Some("user-auth-1")),
                job("must-not-leak", owner),
            ],
        ));
        let gateway = Harness::start(Some(repository.clone())).await;
        let (status, body) = response(gateway.get(1, "")).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(body.get("items").is_none());
        assert!(!body.to_string().contains("legitimate-row"));
        assert!(!body.to_string().contains("must-not-leak"));
        assert_no_private_fields(&body);
        assert_eq!(repository.calls(), vec![("user-auth-1".into(), 50)]);
    }
}

#[tokio::test]
async fn gateway_recharge_recoveries_rejects_invalid_limits_without_repository_calls() {
    let repository = Arc::new(RecoveryRepository::new(ReadMode::OwnerFiltered, vec![]));
    let gateway = Harness::start(Some(repository.clone())).await;
    for limit in ["0", "201", "-1", "1.5", "invalid", "184467440737095516160"] {
        let (status, body) = response(gateway.get(1, &format!("?limit={limit}"))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "limit={limit}");
        assert!(body.get("items").is_none());
    }
    assert!(repository.calls().is_empty());
    let (status, body) = response(gateway.get(1, "?limit=200")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"items":[],"limit":200}));
    assert_eq!(repository.calls(), vec![("user-auth-1".into(), 200)]);
    // The shared wallet query parser treats an empty value as an omitted limit.
    let (status, body) = response(gateway.get(1, "?limit=")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({"items":[],"limit":50}));
    assert_eq!(
        repository.calls(),
        vec![("user-auth-1".into(), 200), ("user-auth-1".into(), 50)]
    );
}

#[tokio::test]
async fn gateway_recharge_recoveries_returns_unavailable_for_missing_unsupported_or_failed_backend()
{
    let gateway = Harness::start(None).await;
    let (status, body) = response(gateway.get(1, "")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_no_private_fields(&body);
    for supported in [false, true] {
        let mut repository = RecoveryRepository::new(ReadMode::Fail, vec![]);
        repository.supported = supported;
        let repository = Arc::new(repository);
        let gateway = Harness::start(Some(repository.clone())).await;
        let (status, body) = response(gateway.get(1, "")).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body.get("items").is_none());
        assert_no_private_fields(&body);
        assert_eq!(repository.calls().len(), usize::from(supported));
    }
}
