use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;

async fn response(state: &AppState, path: &str) -> (StatusCode, serde_json::Value) {
    let response = crate::build_router_with_state(state.clone())
        .oneshot(
            Request::get(path)
                .extension(axum::extract::ConnectInfo(
                    "127.0.0.1:50000".parse::<std::net::SocketAddr>().unwrap(),
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&body).unwrap_or_else(|err| {
            panic!(
                "{path}: {status}: {}: {err}",
                String::from_utf8_lossy(&body)
            )
        }),
    )
}

fn healthy() -> Dependencies {
    Dependencies {
        database: Check::new(true, CheckStatus::Ok),
        redis: Check::new(true, CheckStatus::Ok),
    }
}

#[tokio::test]
async fn readiness_startup_running_closing_and_liveness_contract() {
    let state = AppState::new().unwrap();
    let (status, body) = response(&state, "/readyz").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["lifecycle_status"], "starting");
    assert_eq!(body["warmup_status"], "starting");
    assert_eq!(body["gate_readiness"], false);
    assert_eq!(response(&state, "/health").await.0, StatusCode::OK);

    state.mark_startup_complete(false);
    let (status, body) = response(&state, "/readyz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ready");
    assert_eq!(body["lifecycle_status"], "running");
    assert_eq!(body["warmup_status"], "complete");
    assert_eq!(body["workers"]["usage_queue"]["status"], "disabled");

    state.begin_readiness_shutdown();
    state.mark_startup_complete(false);
    let (status, body) = response(&state, "/readyz").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["status"], "not_ready");
    assert_eq!(body["lifecycle_status"], "closing");
    assert_eq!(body["gate_readiness"], false);
    assert_eq!(response(&state, "/health").await.0, StatusCode::OK);
}

#[tokio::test(start_paused = true)]
async fn readiness_dependency_matrix_has_one_deadline_and_no_error_details() {
    for database_failure in [false, true] {
        for outcome in [CheckStatus::Ok, CheckStatus::Failed, CheckStatus::Timeout] {
            let deadline = Instant::now() + PROBE_TIMEOUT;
            let probe = |affected: bool| async move {
                if affected && outcome == CheckStatus::Timeout {
                    tokio::time::sleep(PROBE_TIMEOUT * 3).await;
                }
                if affected && outcome == CheckStatus::Failed {
                    Err("postgres://private:secret@internal:5432/production")
                } else {
                    Ok(())
                }
            };
            let started = Instant::now();
            let (database, redis) = tokio::join!(
                dependency_probe(true, deadline, probe(database_failure)),
                dependency_probe(true, deadline, probe(!database_failure)),
            );
            let dependencies = Dependencies { database, redis };
            assert_eq!(dependencies.ready(), outcome == CheckStatus::Ok);
            assert!(Instant::now() - started <= PROBE_TIMEOUT);
            assert_eq!(
                if database_failure {
                    database.status
                } else {
                    redis.status
                },
                outcome
            );
            let encoded = serde_json::to_string(&dependencies).unwrap();
            for secret in ["private", "secret", "internal", "5432", "production"] {
                assert!(!encoded.contains(secret));
            }
        }
    }
    // Two stalled dependencies must not turn a one-second deadline into two seconds.
    let started = Instant::now();
    let deadline = started + PROBE_TIMEOUT;
    let (database, redis) = tokio::join!(
        dependency_probe(true, deadline, std::future::pending::<Result<(), ()>>()),
        dependency_probe(true, deadline, std::future::pending::<Result<(), ()>>()),
    );
    assert_eq!(Instant::now() - started, PROBE_TIMEOUT);
    assert_eq!(database.status, CheckStatus::Timeout);
    assert_eq!(redis.status, CheckStatus::Timeout);
}

#[tokio::test(start_paused = true)]
async fn readiness_exact_deadline_is_not_healthy() {
    for delay in [
        PROBE_TIMEOUT - Duration::from_millis(1),
        PROBE_TIMEOUT,
        PROBE_TIMEOUT + Duration::from_millis(1),
    ] {
        let result = dependency_probe(true, Instant::now() + PROBE_TIMEOUT, async {
            tokio::time::sleep(delay).await;
            Ok::<_, ()>(())
        })
        .await;
        assert_eq!(
            result.status,
            if delay < PROBE_TIMEOUT {
                CheckStatus::Ok
            } else {
                CheckStatus::Timeout
            }
        );
    }
}

#[tokio::test(start_paused = true)]
async fn readiness_singleflight_cache_and_cancelled_client_bound_amplification() {
    let readiness = Arc::new(Readiness::default());
    let hits = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    let started = Arc::new(tokio::sync::Notify::new());
    let mut requests = Vec::new();
    for _ in 0..128 {
        let readiness = readiness.clone();
        let hits = hits.clone();
        let release = release.clone();
        let started = started.clone();
        requests.push(tokio::spawn(async move {
            readiness
                .probe(move || async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    started.notify_one();
                    release.notified().await;
                    healthy()
                })
                .await
        }));
    }
    started.notified().await;
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    requests.remove(0).abort();
    release.notify_one();
    for request in requests {
        assert!(request.await.unwrap().unwrap().ready());
    }
    for _ in 0..128 {
        let result = readiness
            .probe(|| async { panic!("a fresh cache must not start another probe") })
            .await;
        assert!(result.unwrap().ready());
    }
    tokio::time::advance(CACHE_TTL).await;
    let hits_probe = hits.clone();
    let failed = readiness
        .probe(move || async move {
            hits_probe.fetch_add(1, Ordering::SeqCst);
            Dependencies {
                database: Check::new(true, CheckStatus::Failed),
                redis: Check::new(true, CheckStatus::Timeout),
            }
        })
        .await
        .unwrap();
    assert!(!failed.ready());
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    assert!(!readiness
        .probe(|| async { panic!("failed results also need negative caching") })
        .await
        .unwrap()
        .ready());
}

#[tokio::test(start_paused = true)]
async fn readiness_closing_interrupts_probe_wait_and_cannot_use_cached_green() {
    let state = AppState::new().unwrap();
    state.mark_startup_complete(false);
    let readiness = state.readiness.clone();
    let started = Arc::new(tokio::sync::Notify::new());
    let signal = started.clone();
    let request = tokio::spawn(async move {
        readiness
            .probe(move || async move {
                signal.notify_one();
                tokio::time::sleep(PROBE_TIMEOUT).await;
                healthy()
            })
            .await
    });
    started.notified().await;
    state.begin_readiness_shutdown();
    assert!(request.await.unwrap().is_none());
    tokio::time::advance(PROBE_TIMEOUT).await;
    let (status, body) = response(&state, "/readyz").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["lifecycle_status"], "closing");
}

#[tokio::test]
async fn readiness_critical_worker_exit_and_restart_are_not_cached() {
    let state = AppState::new().unwrap();
    state.mark_startup_complete(false);
    state
        .readiness
        .required_workers
        .store(COUNTER_WORKER, Ordering::Release);
    assert_eq!(
        response(&state, "/readyz").await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let task = crate::task_runtime::TASK_KEY_USAGE_COUNTER_FLUSH;
    let mut supervisor =
        crate::task_runtime::TaskSupervisor::with_metrics(state.task_supervisor_metrics.clone());
    let release = Arc::new(tokio::sync::Notify::new());
    let release_worker = release.clone();
    supervisor.spawn_named(task, async move {
        release_worker.notified().await;
    });
    assert_eq!(response(&state, "/readyz").await.0, StatusCode::OK);
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(1), async {
        while state.task_supervisor_metrics.snapshot().active_tasks > 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let (status, body) = response(&state, "/readyz").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["workers"]["usage_counter_flush"]["status"], "failed");
    supervisor.spawn_named(task, std::future::pending::<()>());
    assert_eq!(response(&state, "/readyz").await.0, StatusCode::OK);
    supervisor.shutdown().await;
    assert_eq!(
        response(&state, "/readyz").await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(response(&state, "/health").await.0, StatusCode::OK);
}

#[tokio::test]
async fn readiness_usage_shutdown_closes_admission_before_drain() {
    let state = AppState::new().unwrap();
    state.mark_startup_complete(false);
    assert_eq!(response(&state, "/readyz").await.0, StatusCode::OK);
    state
        .shutdown_usage_runtime(Duration::from_secs(1))
        .await
        .unwrap();
    let (status, body) = response(&state, "/readyz").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["lifecycle_status"], "closing");
    assert_eq!(response(&state, "/health").await.0, StatusCode::OK);
}
