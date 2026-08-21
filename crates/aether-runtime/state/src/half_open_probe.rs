use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_data_contracts::repository::half_open_probes::{
    HalfOpenProbeCompletion, HalfOpenProbeOutcome, HalfOpenProbeScope,
};
use uuid::Uuid;

use crate::{DataLayerError, RuntimeLockLease, RuntimeState, RuntimeStateBackendKind};

/// Redis-only coordinator for distributed HalfOpen probe ownership.
///
/// Construction rejects the process-local memory backend so a multi-instance
/// deployment cannot silently degrade to independent local claims.
#[derive(Debug, Clone)]
pub struct DistributedHalfOpenProbeCoordinator {
    runtime: RuntimeState,
}

impl DistributedHalfOpenProbeCoordinator {
    pub fn new(runtime: RuntimeState) -> Result<Self, DataLayerError> {
        if runtime.backend_kind() != RuntimeStateBackendKind::Redis {
            return Err(DataLayerError::InvalidConfiguration(
                "distributed half-open probes require the Redis runtime backend; memory is fail-closed"
                    .to_string(),
            ));
        }
        Ok(Self { runtime })
    }

    pub async fn try_acquire(
        &self,
        scope: HalfOpenProbeScope,
        owner: &str,
        ttl: Duration,
    ) -> Result<Option<HalfOpenProbeLease>, DataLayerError> {
        scope.validate()?;
        let key = scope.coordination_key()?;
        let lease = self.runtime.lock_try_acquire(&key, owner, ttl).await?;
        Ok(lease.map(|inner| HalfOpenProbeLease {
            scope,
            expires_at_unix_ms: expiry_from_now(inner.ttl_ms),
            inner,
        }))
    }

    /// Renew only while Redis still recognizes the exact owner token.
    pub async fn renew(
        &self,
        lease: &mut HalfOpenProbeLease,
        ttl: Duration,
    ) -> Result<bool, DataLayerError> {
        if lease.is_expired_locally() {
            return Ok(false);
        }
        let renewed = self.runtime.lock_renew(&lease.inner, ttl).await?;
        if renewed {
            let ttl_ms = duration_ms(ttl);
            lease.inner.ttl_ms = ttl_ms;
            lease.expires_at_unix_ms = expiry_from_now(ttl_ms);
        }
        Ok(renewed)
    }

    /// Release without authorizing a SQL completion.
    pub async fn release(&self, lease: &HalfOpenProbeLease) -> Result<bool, DataLayerError> {
        if lease.is_expired_locally() {
            return Ok(false);
        }
        self.runtime.lock_release(&lease.inner).await
    }

    /// Validate and extend a live lease before the durable completion CAS.
    ///
    /// The Redis lease remains held until the caller has committed the circuit
    /// transition and completion audit. This prevents a second probe from
    /// entering between authorization and durable completion.
    pub async fn authorize_completion(
        &self,
        lease: &mut HalfOpenProbeLease,
        durable_fencing_token: u64,
        completing_ttl: Duration,
    ) -> Result<Option<HalfOpenProbeCompletionPermit>, DataLayerError> {
        if durable_fencing_token == 0 {
            return Err(DataLayerError::InvalidInput(
                "half-open probe durable fencing token must be positive".to_string(),
            ));
        }
        if lease.is_expired_locally() {
            return Ok(None);
        }
        if !self.renew(lease, completing_ttl).await? {
            return Ok(None);
        }
        Ok(Some(HalfOpenProbeCompletionPermit {
            scope: lease.scope.clone(),
            owner: lease.inner.owner.clone(),
            fencing_token: durable_fencing_token,
        }))
    }
}

pub struct HalfOpenProbeLease {
    scope: HalfOpenProbeScope,
    inner: RuntimeLockLease,
    expires_at_unix_ms: u64,
}

impl std::fmt::Debug for HalfOpenProbeLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HalfOpenProbeLease")
            .field("scope", &self.scope)
            .field("owner", &self.inner.owner)
            .field("fencing_token", &self.inner.fencing_token)
            .field("ttl_ms", &self.inner.ttl_ms)
            .field("expires_at_unix_ms", &self.expires_at_unix_ms)
            .finish_non_exhaustive()
    }
}

impl HalfOpenProbeLease {
    pub fn scope(&self) -> &HalfOpenProbeScope {
        &self.scope
    }

    pub fn owner(&self) -> &str {
        &self.inner.owner
    }

    pub fn fencing_token(&self) -> u64 {
        self.inner.fencing_token
    }

    pub fn expires_at_unix_ms(&self) -> u64 {
        self.expires_at_unix_ms
    }

    pub fn ttl_ms(&self) -> u64 {
        self.inner.ttl_ms
    }

    fn is_expired_locally(&self) -> bool {
        unix_time_ms() >= self.expires_at_unix_ms
    }
}

#[derive(Debug)]
pub struct HalfOpenProbeCompletionPermit {
    scope: HalfOpenProbeScope,
    owner: String,
    fencing_token: u64,
}

impl HalfOpenProbeCompletionPermit {
    pub fn fencing_token(&self) -> u64 {
        self.fencing_token
    }

    pub fn into_completion(
        self,
        outcome: HalfOpenProbeOutcome,
        completed_at_unix_ms: u64,
    ) -> Result<HalfOpenProbeCompletion, DataLayerError> {
        let completion = HalfOpenProbeCompletion {
            completion_id: Uuid::new_v4().to_string(),
            scope: self.scope,
            owner: self.owner,
            fencing_token: self.fencing_token,
            completed_at_unix_ms,
            outcome,
        };
        completion.validate()?;
        Ok(completion)
    }
}

fn duration_ms(ttl: Duration) -> u64 {
    ttl.as_millis().try_into().unwrap_or(u64::MAX)
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn expiry_from_now(ttl_ms: u64) -> u64 {
    unix_time_ms().saturating_add(ttl_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemoryRuntimeStateConfig, RedisClientConfig};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::process::{Child, Command, Stdio};
    use std::sync::Arc;
    use tokio::sync::Barrier;

    #[test]
    fn memory_backend_is_rejected_fail_closed() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let error = DistributedHalfOpenProbeCoordinator::new(runtime)
            .expect_err("memory cannot coordinate distributed probes");
        assert!(error.to_string().contains("memory is fail-closed"));
    }

    #[test]
    fn completion_permit_rejects_zero_completion_timestamp() {
        let permit = HalfOpenProbeCompletionPermit {
            scope: HalfOpenProbeScope::new("key-1", "openai").expect("scope"),
            owner: "node-1".to_string(),
            fencing_token: 1,
        };
        assert!(permit
            .into_completion(HalfOpenProbeOutcome::Succeeded, 0)
            .is_err());
    }

    #[test]
    fn lease_debug_output_redacts_owner_token() {
        let lease = HalfOpenProbeLease {
            scope: HalfOpenProbeScope::new("key-1", "openai").expect("scope"),
            inner: RuntimeLockLease {
                key: "opaque-key".to_string(),
                owner: "node-1".to_string(),
                token: "do-not-log-owner-token".to_string(),
                fencing_token: 7,
                ttl_ms: 1_000,
            },
            expires_at_unix_ms: 2_000,
        };
        let debug = format!("{lease:?}");
        assert!(!debug.contains("do-not-log-owner-token"));
        assert!(debug.contains("node-1"));
        assert!(debug.contains("7"));
    }

    #[tokio::test]
    async fn redis_claims_are_exclusive_expiring_and_monotonically_fenced() {
        let Some(mut server) = required_test_redis_server() else {
            return;
        };
        let runtime = RuntimeState::redis(
            RedisClientConfig {
                url: server.url(),
                key_prefix: Some(format!("half-open-test-{}", std::process::id())),
            },
            Some(1_000),
        )
        .await
        .expect("runtime should connect");
        let coordinator =
            DistributedHalfOpenProbeCoordinator::new(runtime).expect("redis coordinator");
        let scope = HalfOpenProbeScope::new("key-1", "openai").expect("scope");

        let mut first = coordinator
            .try_acquire(scope.clone(), "node-a", Duration::from_secs(1))
            .await
            .expect("first acquire")
            .expect("first lease");
        assert_eq!(first.owner(), "node-a");
        assert!(first.expires_at_unix_ms() > unix_time_ms());
        assert!(coordinator
            .try_acquire(scope.clone(), "node-b", Duration::from_secs(1))
            .await
            .expect("contended acquire")
            .is_none());
        let first_fence = first.fencing_token();
        let permit = coordinator
            .authorize_completion(&mut first, 41, Duration::from_secs(1))
            .await
            .expect("authorize completion")
            .expect("live lease should authorize");
        assert_eq!(permit.fencing_token(), 41);
        assert!(coordinator.release(&first).await.expect("release"));

        let mut second = coordinator
            .try_acquire(scope.clone(), "node-b", Duration::from_millis(30))
            .await
            .expect("second acquire")
            .expect("second lease");
        assert!(second.fencing_token() > first_fence);
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(coordinator
            .authorize_completion(&mut second, 42, Duration::from_secs(1))
            .await
            .expect("expired authorization")
            .is_none());
        server.stop();
    }

    #[tokio::test]
    async fn independent_redis_clients_admit_exactly_one_barrier_contender() {
        let Some(mut server) = required_test_redis_server() else {
            return;
        };
        let prefix = format!("half-open-barrier-test-{}", std::process::id());
        let first = RuntimeState::redis(
            RedisClientConfig {
                url: server.url(),
                key_prefix: Some(prefix.clone()),
            },
            Some(1_000),
        )
        .await
        .expect("first independent Redis client");
        let second = RuntimeState::redis(
            RedisClientConfig {
                url: server.url(),
                key_prefix: Some(prefix),
            },
            Some(1_000),
        )
        .await
        .expect("second independent Redis client");
        let first = DistributedHalfOpenProbeCoordinator::new(first).expect("first coordinator");
        let second = DistributedHalfOpenProbeCoordinator::new(second).expect("second coordinator");
        let scope = HalfOpenProbeScope::new("key-barrier", "openai").expect("scope");
        let barrier = Arc::new(Barrier::new(3));

        let first_task = {
            let barrier = Arc::clone(&barrier);
            let scope = scope.clone();
            let coordinator = first.clone();
            tokio::spawn(async move {
                barrier.wait().await;
                coordinator
                    .try_acquire(scope, "node-a", Duration::from_secs(1))
                    .await
            })
        };
        let second_task = {
            let barrier = Arc::clone(&barrier);
            let coordinator = second.clone();
            tokio::spawn(async move {
                barrier.wait().await;
                coordinator
                    .try_acquire(scope, "node-b", Duration::from_secs(1))
                    .await
            })
        };
        barrier.wait().await;
        let mut claims = vec![
            first_task.await.expect("first task").expect("first claim"),
            second_task
                .await
                .expect("second task")
                .expect("second claim"),
        ];
        assert_eq!(claims.iter().filter(|claim| claim.is_some()).count(), 1);
        let winner = claims
            .drain(..)
            .find_map(|claim| claim)
            .expect("one winner");

        let stale = HalfOpenProbeLease {
            scope: winner.scope.clone(),
            inner: RuntimeLockLease {
                key: winner.inner.key.clone(),
                owner: winner.inner.owner.clone(),
                token: "stale-owner-token".to_string(),
                fencing_token: winner.inner.fencing_token,
                ttl_ms: winner.inner.ttl_ms,
            },
            expires_at_unix_ms: winner.expires_at_unix_ms,
        };
        assert!(!first.release(&stale).await.expect("stale release CAS"));
        assert!(first.release(&winner).await.expect("winner release"));
        server.stop();
    }

    fn required_test_redis_server() -> Option<TestRedisServer> {
        let server = TestRedisServer::start();
        if server.is_none() {
            let required = std::env::var_os("CI").is_some()
                || std::env::var("AETHER_REQUIRE_REDIS_TESTS")
                    .ok()
                    .is_some_and(|value| value == "1");
            assert!(
                !required,
                "redis-server is required for distributed half-open probe tests"
            );
            eprintln!(
                "skipping distributed half-open probe Redis test: redis-server unavailable; set AETHER_REQUIRE_REDIS_TESTS=1 to make this fatal"
            );
        }
        server
    }

    struct TestRedisServer {
        child: Option<Child>,
        port: u16,
    }

    impl TestRedisServer {
        fn start() -> Option<Self> {
            let port = TcpListener::bind("127.0.0.1:0")
                .ok()?
                .local_addr()
                .ok()?
                .port();
            let binary = std::env::var("AETHER_REDIS_SERVER_BIN")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "redis-server".to_string());
            let child = Command::new(binary)
                .arg("--save")
                .arg("")
                .arg("--appendonly")
                .arg("no")
                .arg("--port")
                .arg(port.to_string())
                .arg("--bind")
                .arg("127.0.0.1")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .ok()?;
            let mut server = Self {
                child: Some(child),
                port,
            };
            for _ in 0..100 {
                if redis_ping(port) {
                    return Some(server);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            server.stop();
            None
        }

        fn url(&self) -> String {
            format!("redis://127.0.0.1:{}/0", self.port)
        }

        fn stop(&mut self) {
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    impl Drop for TestRedisServer {
        fn drop(&mut self) {
            self.stop();
        }
    }

    fn redis_ping(port: u16) -> bool {
        let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
            return false;
        };
        if stream.write_all(b"*1\r\n$4\r\nPING\r\n").is_err() {
            return false;
        }
        let mut buffer = [0_u8; 16];
        let Ok(len) = stream.read(&mut buffer) else {
            return false;
        };
        buffer[..len].starts_with(b"+PONG")
    }
}
