//! Fenced ownership for the one request allowed through a due key circuit.
//!
//! The scheduler keeps an open circuit excluded until its probe deadline.  At
//! that deadline every replica can observe the same durable catalog row, so the
//! final owner claim has to happen immediately before the upstream send.

use std::time::Duration;

use aether_runtime_state::{RuntimeLockLease, RuntimeState};
use chrono::DateTime;
use serde_json::Value;
use sha2::{Digest, Sha256};

const HALF_OPEN_PROBE_LOCK_PREFIX: &str = "aether:half-open-probe:v1";
pub(crate) const HALF_OPEN_PROBE_LEASE_TTL: Duration = Duration::from_secs(30);

/// A successful claim.  The fencing token is supplied by RuntimeState and
/// strictly increases for a given distributed lock key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HalfOpenProbeClaim {
    lease: RuntimeLockLease,
}

impl HalfOpenProbeClaim {
    pub(crate) fn fencing_token(&self) -> u64 {
        self.lease.fencing_token
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HalfOpenProbeClaimOutcome {
    /// The candidate's circuit is closed or its open cooldown has not elapsed.
    NotRequired,
    /// This node owns the one due probe.
    Acquired(HalfOpenProbeClaim),
    /// Another owner holds the probe lease.
    Busy,
    /// Runtime coordination could not establish a safe claim.
    Unavailable,
}

pub(crate) async fn try_acquire_half_open_probe(
    runtime_state: &RuntimeState,
    owner: &str,
    provider_key_id: &str,
    api_format: &str,
    circuit_breaker_by_format: Option<&Value>,
    now_unix_secs: u64,
) -> HalfOpenProbeClaimOutcome {
    if !half_open_probe_is_due(circuit_breaker_by_format, api_format, now_unix_secs) {
        return HalfOpenProbeClaimOutcome::NotRequired;
    }

    let Some(lock_key) = half_open_probe_lock_key(provider_key_id, api_format) else {
        return HalfOpenProbeClaimOutcome::Unavailable;
    };
    let owner = owner.trim();
    if owner.is_empty() {
        return HalfOpenProbeClaimOutcome::Unavailable;
    }

    match runtime_state
        .lock_try_acquire(&lock_key, owner, HALF_OPEN_PROBE_LEASE_TTL)
        .await
    {
        Ok(Some(lease)) => HalfOpenProbeClaimOutcome::Acquired(HalfOpenProbeClaim { lease }),
        Ok(None) => HalfOpenProbeClaimOutcome::Busy,
        Err(_) => HalfOpenProbeClaimOutcome::Unavailable,
    }
}

/// Extends an owned probe lease.  A failed compare-and-expire is deliberately
/// reported as false so the caller stops before it can send as a stale owner.
pub(crate) async fn renew_half_open_probe(
    runtime_state: &RuntimeState,
    claim: &HalfOpenProbeClaim,
) -> bool {
    runtime_state
        .lock_renew(&claim.lease, HALF_OPEN_PROBE_LEASE_TTL)
        .await
        .unwrap_or(false)
}

/// Releases only the matching lease token.  A failed compare-and-delete is
/// intentionally not retried by the request path because the lease may already
/// belong to a newer fenced owner.
pub(crate) async fn release_half_open_probe(
    runtime_state: &RuntimeState,
    claim: HalfOpenProbeClaim,
) -> bool {
    runtime_state
        .lock_release(&claim.lease)
        .await
        .unwrap_or(false)
}

pub(crate) fn half_open_probe_is_due(
    circuit_breaker_by_format: Option<&Value>,
    api_format: &str,
    now_unix_secs: u64,
) -> bool {
    let api_format = api_format.trim();
    if api_format.is_empty() {
        return false;
    }
    let Some(circuit) = circuit_breaker_by_format
        .and_then(Value::as_object)
        .and_then(|formats| formats.get(api_format))
        .and_then(Value::as_object)
    else {
        return false;
    };
    if !circuit
        .get("open")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return false;
    }
    circuit_probe_deadline_unix_secs(circuit).is_some_and(|deadline| now_unix_secs >= deadline)
}

fn circuit_probe_deadline_unix_secs(circuit: &serde_json::Map<String, Value>) -> Option<u64> {
    circuit
        .get("next_probe_at_unix_secs")
        .and_then(Value::as_u64)
        .or_else(|| {
            circuit
                .get("next_probe_at")
                .and_then(Value::as_str)
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .and_then(|value| u64::try_from(value.timestamp()).ok())
        })
}

fn half_open_probe_lock_key(provider_key_id: &str, api_format: &str) -> Option<String> {
    let provider_key_id = provider_key_id.trim();
    let api_format = api_format.trim();
    if provider_key_id.is_empty() || api_format.is_empty() {
        return None;
    }
    Some(format!(
        "{HALF_OPEN_PROBE_LOCK_PREFIX}:{}",
        opaque_pair(provider_key_id, api_format)
    ))
}

fn opaque_pair(provider_key_id: &str, api_format: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(provider_key_id.as_bytes());
    digest.update([0]);
    digest.update(api_format.as_bytes());
    let digest = digest.finalize();
    let mut result = String::with_capacity(digest.len().saturating_mul(2));
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(result, "{byte:02x}");
    }
    result
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aether_runtime_state::{MemoryRuntimeStateConfig, RedisClientConfig, RuntimeState};
    use aether_testkit::ManagedRedisServer;
    use serde_json::json;

    use super::{
        half_open_probe_is_due, release_half_open_probe, renew_half_open_probe,
        try_acquire_half_open_probe, HalfOpenProbeClaimOutcome,
    };

    fn due_circuit() -> serde_json::Value {
        json!({"openai:chat": {"open": true, "next_probe_at_unix_secs": 100}})
    }

    #[test]
    fn only_open_circuits_with_elapsed_deadlines_need_a_probe() {
        assert!(half_open_probe_is_due(
            Some(&due_circuit()),
            "openai:chat",
            100
        ));
        assert!(!half_open_probe_is_due(
            Some(&due_circuit()),
            "openai:chat",
            99
        ));
        assert!(!half_open_probe_is_due(
            Some(&json!({"openai:chat": {"open": true}})),
            "openai:chat",
            100
        ));
        assert!(!half_open_probe_is_due(
            Some(&json!({"openai:chat": {"open": false, "next_probe_at_unix_secs": 1}})),
            "openai:chat",
            100
        ));
    }

    #[tokio::test]
    async fn local_runtime_claims_are_exclusive_and_fenced() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let circuit = due_circuit();
        let first = try_acquire_half_open_probe(
            &runtime,
            "node-a",
            "key-a",
            "openai:chat",
            Some(&circuit),
            100,
        )
        .await;
        let HalfOpenProbeClaimOutcome::Acquired(first) = first else {
            panic!("first owner should acquire the probe");
        };
        assert!(first.fencing_token() > 0);
        assert!(matches!(
            try_acquire_half_open_probe(
                &runtime,
                "node-b",
                "key-a",
                "openai:chat",
                Some(&circuit),
                100,
            )
            .await,
            HalfOpenProbeClaimOutcome::Busy
        ));
        assert!(renew_half_open_probe(&runtime, &first).await);
        assert!(release_half_open_probe(&runtime, first).await);

        let second = try_acquire_half_open_probe(
            &runtime,
            "node-b",
            "key-a",
            "openai:chat",
            Some(&circuit),
            100,
        )
        .await;
        let HalfOpenProbeClaimOutcome::Acquired(second) = second else {
            panic!("next owner should acquire the released probe");
        };
        assert!(second.fencing_token() > 1);
        assert!(release_half_open_probe(&runtime, second).await);
    }

    #[tokio::test]
    async fn api_formats_have_independent_probe_claims() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let circuit = json!({
            "openai:chat": {"open": true, "next_probe_at_unix_secs": 100},
            "openai:responses": {"open": true, "next_probe_at_unix_secs": 100}
        });
        assert!(matches!(
            try_acquire_half_open_probe(
                &runtime,
                "node-a",
                "key-a",
                "openai:chat",
                Some(&circuit),
                100,
            )
            .await,
            HalfOpenProbeClaimOutcome::Acquired(_)
        ));
        assert!(matches!(
            try_acquire_half_open_probe(
                &runtime,
                "node-b",
                "key-a",
                "openai:responses",
                Some(&circuit),
                100,
            )
            .await,
            HalfOpenProbeClaimOutcome::Acquired(_)
        ));
    }

    #[tokio::test]
    #[ignore = "requires a shared isolated Redis server"]
    async fn redis_two_gateway_probe_claim_allows_one_owner_and_rejects_stale_operations() {
        let redis_url = std::env::var("AETHER_TEST_REDIS_URL")
            .ok()
            .filter(|url| !url.trim().is_empty());
        let managed_redis = if redis_url.is_none() {
            Some(
                ManagedRedisServer::start()
                    .await
                    .expect("ignored Redis probe test requires an isolated Redis server"),
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
        let key_prefix = format!("aether-half-open-probe-test-{}", uuid::Uuid::new_v4());
        let runtime_config = || RedisClientConfig {
            url: redis_url.clone(),
            key_prefix: Some(key_prefix.clone()),
        };
        let first_gateway = crate::AppState::new()
            .expect("first gateway state should build")
            .with_runtime_state(Arc::new(
                RuntimeState::redis(runtime_config(), Some(1_000))
                    .await
                    .expect("first RuntimeState should connect to Redis"),
            ));
        let second_gateway = crate::AppState::new()
            .expect("second gateway state should build")
            .with_runtime_state(Arc::new(
                RuntimeState::redis(runtime_config(), Some(1_000))
                    .await
                    .expect("second RuntimeState should connect to the same Redis namespace"),
            ));
        let circuit = due_circuit();

        let (first, second) = tokio::join!(
            try_acquire_half_open_probe(
                first_gateway.runtime_state(),
                "node-a",
                "key-a",
                "openai:chat",
                Some(&circuit),
                100,
            ),
            try_acquire_half_open_probe(
                second_gateway.runtime_state(),
                "node-b",
                "key-a",
                "openai:chat",
                Some(&circuit),
                100,
            )
        );
        let (owner, next_owner, claim) = match (first, second) {
            (HalfOpenProbeClaimOutcome::Acquired(claim), HalfOpenProbeClaimOutcome::Busy) => (
                first_gateway.runtime_state(),
                second_gateway.runtime_state(),
                claim,
            ),
            (HalfOpenProbeClaimOutcome::Busy, HalfOpenProbeClaimOutcome::Acquired(claim)) => (
                second_gateway.runtime_state(),
                first_gateway.runtime_state(),
                claim,
            ),
            _ => panic!("two gateways sharing Redis must elect exactly one half-open probe owner"),
        };
        let stale = claim.clone();
        assert!(release_half_open_probe(owner, claim).await);

        let second = try_acquire_half_open_probe(
            next_owner,
            "node-next",
            "key-a",
            "openai:chat",
            Some(&circuit),
            100,
        )
        .await;
        let HalfOpenProbeClaimOutcome::Acquired(second) = second else {
            panic!("the other gateway should acquire after the winning gateway releases");
        };
        assert!(second.fencing_token() > stale.fencing_token());
        assert!(!renew_half_open_probe(owner, &stale).await);
        assert!(!release_half_open_probe(owner, stale).await);
        assert!(renew_half_open_probe(next_owner, &second).await);
        assert!(release_half_open_probe(next_owner, second).await);
        drop(managed_redis);
    }
}
