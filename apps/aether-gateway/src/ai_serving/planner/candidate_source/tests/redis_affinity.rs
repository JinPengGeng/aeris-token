use std::collections::BTreeSet;
use std::sync::Arc;

use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
use aether_runtime_state::{RedisClientConfig, RuntimeState};
use aether_scheduler_core::{
    build_scheduler_affinity_cache_key_for_api_key_id_with_client_session, ClientSessionAffinity,
};
use aether_testkit::ManagedRedisServer;

use crate::ai_serving::planner::candidate_affinity_cache::remember_scheduler_affinity_for_candidate;
use crate::ai_serving::PlannerAppState;
use crate::data::GatewayDataState;
use crate::AppState;

use super::{
    standard_candidate_row, unrestricted_auth_snapshot, LocalCandidatePreselectionKeyMode,
    LocalCandidatePreselectionPageCursor,
};

const API_FORMAT: &str = "openai:chat";
const MODEL: &str = "gpt-5";
const CANDIDATE_COUNT: usize = 257;

fn candidate_key(
    candidate: &aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate,
) -> String {
    format!(
        "{}:{}:{}:{}",
        candidate.provider_id, candidate.endpoint_id, candidate.key_id, candidate.model_id
    )
}

fn state_with_candidate_rows(
    repository: Arc<InMemoryMinimalCandidateSelectionReadRepository>,
    runtime: Arc<RuntimeState>,
) -> AppState {
    AppState::new()
        .expect("gateway state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_minimal_candidate_selection_reader_for_tests(repository),
        )
        .with_runtime_state(runtime)
}

async fn wait_for_affinity_target(runtime: &RuntimeState, key: &str, expected_provider_id: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let matches = runtime
                .kv_get(key)
                .await
                .ok()
                .flatten()
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                .and_then(|value| {
                    value
                        .get("provider_id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
                .as_deref()
                == Some(expected_provider_id);
            if matches {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("affinity runtime write should become visible");
}

#[tokio::test]
#[ignore = "requires a shared isolated Redis server"]
async fn redis_two_gateways_share_affinity_across_selector_pages_without_duplicates_or_omissions() {
    let redis_url = std::env::var("AETHER_TEST_REDIS_URL")
        .ok()
        .filter(|url| !url.trim().is_empty());
    let managed_redis = if redis_url.is_none() {
        Some(
            ManagedRedisServer::start()
                .await
                .expect("ignored Redis affinity test requires an isolated Redis server"),
        )
    } else {
        None
    };
    let redis_url = redis_url.unwrap_or_else(|| {
        managed_redis
            .as_ref()
            .expect("managed Redis server should be available")
            .redis_url()
            .to_owned()
    });
    let key_prefix = format!("aether-planner-affinity-test-{}", uuid::Uuid::new_v4());
    let runtime = || async {
        Arc::new(
            RuntimeState::redis(
                RedisClientConfig {
                    url: redis_url.clone(),
                    key_prefix: Some(key_prefix.clone()),
                },
                Some(1_000),
            )
            .await
            .expect("gateway RuntimeState should connect to the shared Redis namespace"),
        )
    };
    let rows = (0..CANDIDATE_COUNT)
        .map(|index| standard_candidate_row(&format!("provider-{index:03}"), API_FORMAT, 0))
        .collect::<Vec<_>>();
    let expected_keys = rows
        .iter()
        .map(|row| {
            format!(
                "{}:{}:{}:{}",
                row.provider_id, row.endpoint_id, row.key_id, row.model_id
            )
        })
        .collect::<BTreeSet<_>>();
    let repository = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(rows));
    let first_runtime = runtime().await;
    let second_runtime = runtime().await;
    let first_gateway =
        state_with_candidate_rows(Arc::clone(&repository), Arc::clone(&first_runtime));
    let second_gateway = state_with_candidate_rows(repository, Arc::clone(&second_runtime));
    let auth_snapshot = unrestricted_auth_snapshot();
    let session_affinity = ClientSessionAffinity::from_session_key("redis-shared-session");
    let second_policy =
        crate::system_features::ModelDirectivePolicySnapshot::load(&second_gateway).await;
    let mut baseline_selector = LocalCandidatePreselectionPageCursor::new(
        PlannerAppState::new(&second_gateway),
        &second_policy,
        API_FORMAT,
        MODEL,
        None,
        true,
        None,
        &auth_snapshot,
        None,
        Some(&session_affinity),
        None,
        true,
        LocalCandidatePreselectionKeyMode::ProviderEndpointKeyModel,
        true,
        Some("redis-affinity-gateway-b-baseline"),
    )
    .await;
    let baseline_page = baseline_selector
        .next_page()
        .await
        .expect("baseline selector should succeed")
        .expect("baseline selector should return candidates");
    assert_eq!(baseline_page.candidates.len(), 256);
    let baseline_first = candidate_key(&baseline_page.candidates[0]);
    let first_target = baseline_page
        .candidates
        .last()
        .expect("first page should have a non-baseline first target");
    let second_target = baseline_page
        .candidates
        .get(baseline_page.candidates.len() - 2)
        .expect("first page should have a second target");
    assert_ne!(candidate_key(first_target), baseline_first);
    assert_ne!(candidate_key(second_target), baseline_first);
    while baseline_selector.next_page().await.unwrap().is_some() {}
    let affinity_key = build_scheduler_affinity_cache_key_for_api_key_id_with_client_session(
        &auth_snapshot.api_key_id,
        API_FORMAT,
        MODEL,
        Some(&session_affinity),
    )
    .expect("explicit affinity should produce a cache key");

    remember_scheduler_affinity_for_candidate(
        PlannerAppState::new(&first_gateway),
        Some(&auth_snapshot),
        Some(&session_affinity),
        API_FORMAT,
        MODEL,
        first_target,
    );
    wait_for_affinity_target(&first_runtime, &affinity_key, &first_target.provider_id).await;

    let mut warm_selector = LocalCandidatePreselectionPageCursor::new(
        PlannerAppState::new(&second_gateway),
        &second_policy,
        API_FORMAT,
        MODEL,
        None,
        true,
        None,
        &auth_snapshot,
        None,
        Some(&session_affinity),
        None,
        true,
        LocalCandidatePreselectionKeyMode::ProviderEndpointKeyModel,
        true,
        Some("redis-affinity-gateway-b-warm"),
    )
    .await;
    let warm_page = warm_selector.next_page().await.unwrap().unwrap();
    assert_eq!(
        candidate_key(&warm_page.candidates[0]),
        candidate_key(first_target)
    );
    while warm_selector.next_page().await.unwrap().is_some() {}

    remember_scheduler_affinity_for_candidate(
        PlannerAppState::new(&first_gateway),
        Some(&auth_snapshot),
        Some(&session_affinity),
        API_FORMAT,
        MODEL,
        second_target,
    );
    wait_for_affinity_target(&first_runtime, &affinity_key, &second_target.provider_id).await;

    let mut second_selector = LocalCandidatePreselectionPageCursor::new(
        PlannerAppState::new(&second_gateway),
        &second_policy,
        API_FORMAT,
        MODEL,
        None,
        true,
        None,
        &auth_snapshot,
        None,
        Some(&session_affinity),
        None,
        true,
        LocalCandidatePreselectionKeyMode::ProviderEndpointKeyModel,
        true,
        Some("redis-affinity-gateway-b"),
    )
    .await;
    let mut pages = Vec::new();
    while let Some(page) = second_selector
        .next_page()
        .await
        .expect("second gateway selector should succeed")
    {
        pages.push(page);
    }

    assert_eq!(
        pages
            .iter()
            .map(|page| page.candidates.len())
            .collect::<Vec<_>>(),
        [256, 1]
    );
    let selected = pages
        .iter()
        .flat_map(|page| page.candidates.iter())
        .map(candidate_key)
        .collect::<Vec<_>>();
    assert_eq!(selected.first(), Some(&candidate_key(second_target)));
    assert_eq!(selected.len(), CANDIDATE_COUNT);
    assert_eq!(
        selected.iter().cloned().collect::<BTreeSet<_>>(),
        expected_keys
    );
    drop(managed_redis);
}
