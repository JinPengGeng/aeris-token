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

    /// Atomically consume a live Redis lease before attempting fenced SQL completion.
    ///
    /// A false result means the token was lost, replaced, or expired. Callers
    /// must not write completion state in that case.
    pub async fn consume_for_completion(
        &self,
        lease: HalfOpenProbeLease,
    ) -> Result<Option<HalfOpenProbeCompletionPermit>, DataLayerError> {
        if lease.is_expired_locally() {
            return Ok(None);
        }
        if !self.runtime.lock_release(&lease.inner).await? {
            return Ok(None);
        }
        Ok(Some(HalfOpenProbeCompletionPermit {
            scope: lease.scope,
            owner: lease.inner.owner,
            fencing_token: lease.inner.fencing_token,
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
        let Some(mut server) = TestRedisServer::start() else {
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

        let first = coordinator
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
        assert!(coordinator
            .consume_for_completion(first)
            .await
            .expect("consume")
            .is_some());

        let second = coordinator
            .try_acquire(scope.clone(), "node-b", Duration::from_millis(30))
            .await
            .expect("second acquire")
            .expect("second lease");
        assert!(second.fencing_token() > first_fence);
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(coordinator
            .consume_for_completion(second)
            .await
            .expect("expired consume")
            .is_none());
        server.stop();
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
