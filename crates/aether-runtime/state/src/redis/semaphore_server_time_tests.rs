use super::*;

type Connection = ::redis::aio::MultiplexedConnection;

fn runner(runtime: &RuntimeState) -> &crate::redis::RedisRuntimeRunner {
    let RuntimeStateBackend::Redis(backend) = runtime.backend.as_ref() else {
        panic!("expected Redis backend");
    };
    &backend.runtime
}

async fn runtime(server: &TestRedisServer) -> RuntimeState {
    RuntimeState::redis(
        RedisClientConfig {
            url: server.redis_url.clone(),
            key_prefix: Some("lease-test".to_string()),
        },
        Some(500),
    )
    .await
    .expect("connect isolated Redis runtime")
}

async fn server_time(connection: &mut Connection) -> u64 {
    let (seconds, micros): (u64, u64) = ::redis::cmd("TIME")
        .query_async(connection)
        .await
        .unwrap();
    seconds * 1_000 + micros / 1_000
}

async fn lease_score(connection: &mut Connection, key: &str, token: &str) -> u64 {
    let score: f64 = ::redis::cmd("ZSCORE")
        .arg(format!("lease-test:{key}"))
        .arg(token)
        .query_async(connection)
        .await
        .unwrap();
    score as u64
}

struct ClientClock;

impl ClientClock {
    fn set(now_ms: u64) -> Self {
        assert!(TEST_UNIX_TIME_MS.replace(Some(now_ms)).is_none());
        Self
    }
}

impl Drop for ClientClock {
    fn drop(&mut self) {
        TEST_UNIX_TIME_MS.set(None);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn redis_semaphore_uses_server_time_with_skewed_client_clocks() {
    let Some(server) = TestRedisServer::start().await else {
        return;
    };
    let first = runtime(&server).await;
    let second = runtime(&server).await;
    let mut admin = redis_test_connection(&server.redis_url).await;
    let ttl = 30_000;
    let before = server_time(&mut admin).await;
    {
        let _clock = ClientClock::set(0);
        assert_eq!(unix_time_ms(), 0, "client clock override is active");
        assert_eq!(
            runner(&first)
                .semaphore_try_acquire("clock", 1, "clock", "original", ttl, None)
                .await
                .unwrap(),
            (1, 1)
        );
    }
    let after = server_time(&mut admin).await;
    let score = lease_score(&mut admin, "clock", "original").await;
    assert!((before + ttl..=after + ttl).contains(&score));

    {
        let fast_client_now = after + 86_400_000;
        let _clock = ClientClock::set(fast_client_now);
        assert_eq!(unix_time_ms(), fast_client_now);
        assert_eq!(
            runner(&second)
                .semaphore_live_count("clock", 1, "clock", None)
                .await
                .unwrap(),
            1,
            "a fast gateway clock cannot evict another gateway's lease"
        );
        assert_eq!(
            runner(&second)
                .semaphore_try_acquire("clock", 1, "clock", "other", ttl, None)
                .await
                .unwrap(),
            (0, 1)
        );
        let before = server_time(&mut admin).await;
        assert_eq!(
            runner(&first)
                .semaphore_renew("clock", 1, "clock", "original", ttl, None)
                .await
                .unwrap(),
            1
        );
        let after = server_time(&mut admin).await;
        let score = lease_score(&mut admin, "clock", "original").await;
        assert!((before + ttl..=after + ttl).contains(&score));
    }

    // Keep the key alive but make its lease stale in the server's time domain.
    let expired = server_time(&mut admin).await - 1;
    for key in ["snapshot", "acquire"] {
        ::redis::cmd("ZADD")
            .arg(format!("lease-test:{key}"))
            .arg(expired)
            .arg("original")
            .query_async::<usize>(&mut admin)
            .await
            .unwrap();
    }
    ::redis::cmd("ZADD")
        .arg("lease-test:clock")
        .arg(expired)
        .arg("original")
        .query_async::<usize>(&mut admin)
        .await
        .unwrap();
    {
        let _clock = ClientClock::set(0);
        assert_eq!(
            runner(&second)
                .semaphore_live_count("clock", 1, "snapshot", None)
                .await
                .unwrap(),
            0,
            "snapshot must prune expired leases even with a slow gateway clock"
        );
        assert_eq!(
            runner(&second)
                .semaphore_try_acquire("clock", 1, "acquire", "successor", ttl, None)
                .await
                .unwrap(),
            (1, 1),
            "acquire must reclaim expired capacity even with a slow gateway clock"
        );
        assert_eq!(
            runner(&first)
                .semaphore_renew("clock", 1, "clock", "original", ttl, None)
                .await
                .unwrap(),
            0,
            "a slow gateway clock cannot resurrect an expired token"
        );
        assert_eq!(
            runner(&second)
                .semaphore_live_count("clock", 1, "clock", None)
                .await
                .unwrap(),
            0
        );
    }
}

#[tokio::test]
async fn redis_semaphore_concurrent_instances_keep_capacity_and_token_ownership() {
    let Some(server) = TestRedisServer::start().await else {
        return;
    };
    let mut gates = Vec::new();
    for _ in 0..4 {
        gates.push(
            runtime(&server)
                .await
                .semaphore("parallel", 8, RuntimeSemaphoreConfig::default())
                .unwrap(),
        );
    }
    let mut tasks = tokio::task::JoinSet::new();
    let barrier = Arc::new(tokio::sync::Barrier::new(64));
    for index in 0..64 {
        let gate = gates[index % gates.len()].clone();
        let barrier = Arc::clone(&barrier);
        tasks.spawn(async move {
            barrier.wait().await;
            gate.try_acquire().await
        });
    }
    let mut permits = Vec::new();
    let mut rejected = 0;
    while let Some(result) = tasks.join_next().await {
        match result.unwrap() {
            Ok(permit) => permits.push(permit),
            Err(RuntimeSemaphoreError::Saturated { limit: 8, .. }) => rejected += 1,
            unexpected => panic!("unexpected acquisition: {unexpected:?}"),
        }
    }
    assert_eq!(permits.len(), 8);
    assert_eq!(rejected, 56);
    for gate in &gates {
        assert_eq!(gate.snapshot().await.unwrap().in_flight, 8);
    }
    let rt = runtime(&server).await;
    runner(&rt)
        .semaphore_release("parallel", 8, "admission:parallel", "not-the-owner", None)
        .await
        .unwrap();
    assert_eq!(gates[0].snapshot().await.unwrap().in_flight, 8);
    permits.pop().unwrap().release().await.unwrap();
    let replacement = gates[1].try_acquire().await.unwrap();
    assert_eq!(gates[0].snapshot().await.unwrap().in_flight, 8);
    replacement.release().await.unwrap();
    for permit in permits {
        permit.release().await.unwrap();
    }
    assert_eq!(gates[0].snapshot().await.unwrap().in_flight, 0);
}

#[tokio::test]
async fn redis_semaphore_renews_expires_and_rejects_late_renewal() {
    let Some(server) = TestRedisServer::start().await else {
        return;
    };
    let rt = runtime(&server).await;
    let mut admin = redis_test_connection(&server.redis_url).await;
    let ttl = 1_000;
    assert_eq!(
        runner(&rt)
            .semaphore_try_acquire("expiry", 1, "expiry", "old", ttl, None)
            .await
            .unwrap(),
        (1, 1)
    );
    let initial_score = lease_score(&mut admin, "expiry", "old").await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        runner(&rt)
            .semaphore_renew("expiry", 1, "expiry", "old", ttl, None)
            .await
            .unwrap(),
        1
    );
    assert!(lease_score(&mut admin, "expiry", "old").await > initial_score);
    let pttl: i64 = ::redis::cmd("PTTL")
        .arg("lease-test:expiry")
        .query_async(&mut admin)
        .await
        .unwrap();
    assert!((1..=ttl as i64).contains(&pttl));
    tokio::time::sleep(Duration::from_millis(ttl + 20)).await;
    assert_eq!(
        runner(&rt)
            .semaphore_renew("expiry", 1, "expiry", "old", ttl, None)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        runner(&rt)
            .semaphore_live_count("expiry", 1, "expiry", None)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        runner(&rt)
            .semaphore_try_acquire("expiry", 1, "expiry", "new", ttl, None)
            .await
            .unwrap(),
        (1, 1)
    );
    runner(&rt)
        .semaphore_release("expiry", 1, "expiry", "old", None)
        .await
        .unwrap();
    assert_eq!(
        runner(&rt)
            .semaphore_live_count("expiry", 1, "expiry", None)
            .await
            .unwrap(),
        1,
        "late release must not remove a successor lease"
    );
}

#[tokio::test]
async fn redis_semaphore_time_acl_denial_is_unavailable_and_preserves_leases() {
    let Some(server) = TestRedisServer::start().await else {
        return;
    };
    let mut admin = redis_test_connection(&server.redis_url).await;
    ::redis::cmd("ACL")
        .arg("SETUSER")
        .arg("lease-user")
        .arg("on")
        .arg(">lease-test-password")
        .arg("~*")
        .arg("+@all")
        .query_async::<()>(&mut admin)
        .await
        .unwrap();
    let rt = RuntimeState::redis(
        RedisClientConfig {
            url: format!(
                "redis://lease-user:lease-test-password@127.0.0.1:{}/0",
                server.port
            ),
            key_prefix: Some("lease-test".to_string()),
        },
        Some(500),
    )
    .await
    .unwrap();
    let gate = rt
        .semaphore(
            "acl",
            1,
            RuntimeSemaphoreConfig {
                renew_interval_ms: 20,
                ..RuntimeSemaphoreConfig::default()
            },
        )
        .unwrap();
    let permit = gate.try_acquire().await.unwrap();
    // Include an expired member: TIME denial must happen before pruning it.
    ::redis::cmd("ZADD")
        .arg("lease-test:admission:acl")
        .arg(0)
        .arg("expired")
        .query_async::<usize>(&mut admin)
        .await
        .unwrap();
    ::redis::cmd("ACL")
        .arg("SETUSER")
        .arg("lease-user")
        .arg("-time")
        .query_async::<()>(&mut admin)
        .await
        .unwrap();
    let before: Vec<(String, f64)> = ::redis::cmd("ZRANGE")
        .arg("lease-test:admission:acl")
        .arg(0)
        .arg(-1)
        .arg("WITHSCORES")
        .query_async(&mut admin)
        .await
        .unwrap();
    assert_eq!(before.len(), 2);
    for result in [
        gate.try_acquire().await.map(|_| ()),
        gate.snapshot().await.map(|_| ()),
        gate.state.renew(&permit.token).await,
    ] {
        assert!(
            matches!(result, Err(RuntimeSemaphoreError::Unavailable { .. })),
            "TIME denial must fail closed: {result:?}"
        );
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while aether_runtime::AdmissionPermitHealth::is_healthy(&permit)
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!aether_runtime::AdmissionPermitHealth::is_healthy(&permit));
    let after: Vec<(String, f64)> = ::redis::cmd("ZRANGE")
        .arg("lease-test:admission:acl")
        .arg(0)
        .arg(-1)
        .arg("WITHSCORES")
        .query_async(&mut admin)
        .await
        .unwrap();
    assert_eq!(after, before);
    // Release needs no clock permission and remains usable during recovery.
    permit.release().await.unwrap();
    ::redis::cmd("ACL")
        .arg("SETUSER")
        .arg("lease-user")
        .arg("+time")
        .query_async::<()>(&mut admin)
        .await
        .unwrap();
    assert_eq!(gate.snapshot().await.unwrap().in_flight, 0);
    gate.try_acquire().await.unwrap().release().await.unwrap();
}

#[tokio::test]
async fn redis_semaphore_rejects_inexact_or_overflowing_ttl_before_writes() {
    let Some(server) = TestRedisServer::start().await else {
        return;
    };
    let rt = runtime(&server).await;
    let mut admin = redis_test_connection(&server.redis_url).await;
    for ttl in [0, 1_u64 << 53, i64::MAX as u64 + 1, u64::MAX] {
        for result in [
            runner(&rt)
                .semaphore_try_acquire("range", 1, "range", "invalid", ttl, None)
                .await
                .map(|_| ()),
            runner(&rt)
                .semaphore_renew("range", 1, "range", "invalid", ttl, None)
                .await
                .map(|_| ()),
        ] {
            assert!(matches!(result, Err(RuntimeSemaphoreError::InvalidConfiguration(_))));
        }
    }
    // Individually exact TTL plus server epoch exceeds Lua's exact range.
    for result in [
        runner(&rt)
            .semaphore_try_acquire("range", 1, "range", "invalid", (1_u64 << 53) - 1, None)
            .await
            .map(|_| ()),
        runner(&rt)
            .semaphore_renew("range", 1, "range", "invalid", (1_u64 << 53) - 1, None)
            .await
            .map(|_| ()),
    ] {
        assert!(matches!(result, Err(RuntimeSemaphoreError::Unavailable { .. })));
    }
    let exists: bool = ::redis::cmd("EXISTS")
        .arg("lease-test:range")
        .query_async(&mut admin)
        .await
        .unwrap();
    assert!(!exists, "invalid TTL must not leave a partial lease");
}

#[tokio::test]
async fn redis_semaphore_fails_closed_and_recovers_after_server_restart() {
    let Some(mut server) = TestRedisServer::start().await else {
        return;
    };
    let rt = runtime(&server).await;
    let gate = rt
        .semaphore(
            "restart",
            1,
            RuntimeSemaphoreConfig {
                renew_interval_ms: 20,
                command_timeout_ms: Some(100),
                ..RuntimeSemaphoreConfig::default()
            },
        )
        .unwrap();
    let permit = gate.try_acquire().await.unwrap();
    server.stop();
    assert!(matches!(gate.try_acquire().await, Err(RuntimeSemaphoreError::Unavailable { .. })));
    assert!(matches!(gate.snapshot().await, Err(RuntimeSemaphoreError::Unavailable { .. })));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while aether_runtime::AdmissionPermitHealth::is_healthy(&permit)
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!aether_runtime::AdmissionPermitHealth::is_healthy(&permit));
    server.restart().await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if gate.snapshot().await.is_ok() {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "runtime must reconnect");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(matches!(gate.state.renew(&permit.token).await, Err(RuntimeSemaphoreError::Unavailable { .. })));
    let replacement = gate.try_acquire().await.unwrap();
    permit.release().await.unwrap();
    assert_eq!(gate.snapshot().await.unwrap().in_flight, 1);
    replacement.release().await.unwrap();
}
