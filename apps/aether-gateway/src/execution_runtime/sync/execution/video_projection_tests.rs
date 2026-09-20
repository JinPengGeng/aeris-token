#[derive(Clone, Copy)]
enum VideoProjectionFixture {
    Conflict,
    DatabaseError,
    NoWriter,
}

async fn assert_video_projection_preserves_upstream_success(
    finalize: bool,
    projection: VideoProjectionFixture,
) {
    use aether_data::repository::video_tasks::{
        InMemoryVideoTaskRepository, SqlxVideoTaskRepository,
    };
    use aether_data_contracts::repository::video_tasks::{
        UpsertVideoTask, VideoTaskLookupKey, VideoTaskReadRepository, VideoTaskStatus,
        VideoTaskWriteRepository,
    };
    use aether_runtime_state::{MemoryRuntimeStateConfig, RuntimeQueueStore, RuntimeState};
    use aether_usage_runtime::UsageEvent;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let candidates = Arc::new(InMemoryRequestCandidateRepository::default());
    let usages = Arc::new(InMemoryUsageReadRepository::default());
    let tasks = Arc::new(InMemoryVideoTaskRepository::default());
    let queue = Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_upstream = Arc::clone(&calls);
    let provider_body = json!({"id": "upstream-cas-video", "status": "queued"});
    let upstream_body = if finalize {
        json!({})
    } else {
        provider_body.clone()
    };
    let data = crate::data::GatewayDataState::with_request_candidate_and_usage_repository_for_tests(
        Arc::clone(&candidates),
        usages,
    );
    let data = match projection {
        VideoProjectionFixture::Conflict => {
            data.attach_video_task_repository_for_tests(Arc::clone(&tasks))
        }
        VideoProjectionFixture::DatabaseError => {
            // A closed real SQLx pool deterministically rejects persistence without
            // opening a database connection or relying on a network timeout.
            let pool = sqlx::postgres::PgPoolOptions::new()
                .connect_lazy("postgres://fixture:fixture@127.0.0.1:1/unused")
                .unwrap();
            pool.close().await;
            data.attach_video_task_repository_for_tests(Arc::new(SqlxVideoTaskRepository::new(
                pool,
            )))
        }
        VideoProjectionFixture::NoWriter => data,
    };
    let state = AppState::new()
        .unwrap()
        // AppState binds both foreground and background data to its own runtime
        // queue when replacing data; inject the queue at the owning state.
        .with_runtime_state(Arc::clone(&queue))
        .with_video_task_truth_source_mode(
            crate::video_tasks::VideoTaskTruthSourceMode::RustAuthoritative,
        )
        .with_data_state_for_tests(data)
        .with_usage_runtime_for_tests(UsageRuntimeConfig {
            enabled: true,
            queue_terminal_events: true,
            queue_lifecycle_events: false,
            ..UsageRuntimeConfig::default()
        })
        .with_execution_runtime_sync_override_for_tests(move |plan| {
            calls_for_upstream.fetch_add(1, Ordering::SeqCst);
            Ok(ExecutionResult {
                request_id: plan.request_id.clone(),
                candidate_id: plan.candidate_id.clone(),
                status_code: 200,
                headers: BTreeMap::from([(
                    "content-type".to_string(),
                    "application/json".to_string(),
                )]),
                response_observation: None,
                body: Some(aether_contracts::ResponseBody {
                    json_body: Some(upstream_body.clone()),
                    body_bytes_b64: None,
                }),
                telemetry: Some(aether_contracts::ExecutionTelemetry {
                    ttfb_ms: Some(1),
                    elapsed_ms: Some(2),
                    upstream_bytes: None,
                }),
                error: None,
            })
        });
    let mut plan = test_openai_image_plan(false);
    plan.request_id = "video-upstream-success".to_string();
    plan.candidate_id = Some("video-upstream-success-candidate".to_string());
    plan.url = "https://video.example.invalid/v1/videos".to_string();
    plan.client_api_format = "openai:video".to_string();
    plan.provider_api_format = "openai:video".to_string();
    plan.model_name = Some("sora-2".to_string());
    plan.body =
        aether_contracts::RequestBody::from_json(json!({"model":"sora-2","prompt":"fixture"}));
    let mut context = json!({
        "request_id": plan.request_id, "candidate_id": plan.candidate_id,
        "candidate_index": 0, "retry_index": 0,
        "user_id": "video-user", "api_key_id": "video-api-key",
        "provider_id": plan.provider_id, "endpoint_id": plan.endpoint_id, "key_id": plan.key_id,
        "provider_name": "OpenAI", "model": "sora-2", "mapped_model": "sora-2",
        "client_api_format": "openai:video", "provider_api_format": "openai:video",
        "request_path": "/v1/videos", "request_path_and_query": "/v1/videos",
        "upstream_url": plan.url,
        "local_task_id": "local-cas-video", "task_id": "local-cas-video",
        "local_created_at": 1_700_000_000,
        "original_request_body": {"model":"sora-2", "prompt":"fixture"}
    });
    let seed = state
        .video_tasks
        .prepare_sync_success(
            "openai_video_create_sync_finalize",
            provider_body.as_object().unwrap(),
            context.as_object().unwrap(),
            &plan,
        )
        .unwrap()
        .to_snapshot();
    let stored = tasks.upsert(seed.to_upsert_record()).await.unwrap();
    if finalize {
        let observed = if matches!(projection, VideoProjectionFixture::NoWriter) {
            seed.clone()
        } else {
            seed.with_stored_task(&stored).unwrap()
        };
        context["video_task_row_revision"] = json!(observed.row_revision());
        state.video_tasks.record_snapshot(observed);
    }
    if matches!(projection, VideoProjectionFixture::Conflict) {
        // A competing writer wins after the operation observed its snapshot.
        tasks
            .update_if_active(
                UpsertVideoTask {
                    status: VideoTaskStatus::Processing,
                    progress_percent: 80,
                    ..stored.into()
                },
                None,
            )
            .await
            .unwrap()
            .unwrap();
    }
    let (path, kind, report_kind) = if finalize {
        (
            "/v1/videos/local-cas-video/cancel",
            "openai_video_cancel_sync",
            "openai_video_cancel_sync_finalize",
        )
    } else {
        (
            "/v1/videos",
            "openai_video_create_sync",
            "openai_video_create_sync_finalize",
        )
    };
    let decision = GatewayControlDecision::synthetic(
        path,
        Some("ai_public".to_string()),
        Some("openai".to_string()),
        Some("video".to_string()),
        Some("openai:video".to_string()),
    )
    .with_execution_runtime_candidate(true);
    let result = Box::pin(execute_execution_runtime_sync(
        &state,
        path,
        plan,
        "video-upstream-success-trace",
        &decision,
        kind,
        Some(report_kind.to_string()),
        Some(context),
    ))
    .await;
    match projection {
        VideoProjectionFixture::Conflict => assert!(matches!(
            result,
            Err(GatewayError::Client {
                status: StatusCode::CONFLICT,
                ..
            })
        )),
        VideoProjectionFixture::DatabaseError => {
            assert!(matches!(result, Err(GatewayError::Internal(_))))
        }
        VideoProjectionFixture::NoWriter => {
            assert_eq!(result.unwrap().unwrap().status(), StatusCode::OK)
        }
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "never repeat the completed upstream operation"
    );
    state
        .usage_runtime
        .shutdown(Duration::from_secs(2))
        .await
        .unwrap();
    let page = queue
        .read_stream_page("usage:events", "0-0", 10)
        .await
        .unwrap();
    assert_eq!(
        page.entries.len(),
        1,
        "exactly one terminal accounting event; no abort-guard failure"
    );
    let event = UsageEvent::from_stream_fields(&page.entries[0].fields).unwrap();
    assert_eq!(event.request_id, "video-upstream-success");
    // A successful cancel retains the Cancelled accounting contract even though
    // the upstream operation and its candidate report succeeded with HTTP 200.
    assert_eq!(
        event.event_type,
        if finalize {
            UsageEventType::Cancelled
        } else {
            UsageEventType::Completed
        }
    );
    assert_eq!(event.data.status_code, Some(200));
    // The success report has its own spawned task outside the usage queue drain.
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let recorded = candidates
                .list_by_request_id("video-upstream-success")
                .await
                .unwrap();
            assert_eq!(recorded.len(), 1);
            if recorded[0].status == RequestCandidateStatus::Success {
                assert_eq!(recorded[0].status_code, Some(200));
                assert!(recorded[0].error_type.is_none());
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the upstream success report must complete after a projection failure");
    if matches!(projection, VideoProjectionFixture::Conflict) {
        let winner = tasks
            .find(VideoTaskLookupKey::Id("local-cas-video"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(winner.status, VideoTaskStatus::Processing);
        assert_eq!(winner.progress_percent, 80);
        let cached = state
            .video_tasks
            .snapshot_for_route(Some("openai"), "/v1/videos/local-cas-video")
            .unwrap();
        assert_eq!(cached.row_revision(), winner.row_revision);
        assert_eq!(cached.to_upsert_record().status, winner.status);
    }
}

#[tokio::test]
async fn video_create_conflict_preserves_upstream_success_once() {
    Box::pin(assert_video_projection_preserves_upstream_success(
        false,
        VideoProjectionFixture::Conflict,
    ))
    .await;
}

#[tokio::test]
async fn video_create_database_error_preserves_upstream_success_once() {
    Box::pin(assert_video_projection_preserves_upstream_success(
        false,
        VideoProjectionFixture::DatabaseError,
    ))
    .await;
}

#[tokio::test]
async fn video_finalize_conflict_preserves_upstream_success_once() {
    Box::pin(assert_video_projection_preserves_upstream_success(
        true,
        VideoProjectionFixture::Conflict,
    ))
    .await;
}

#[tokio::test]
async fn video_finalize_database_error_preserves_upstream_success_once() {
    Box::pin(assert_video_projection_preserves_upstream_success(
        true,
        VideoProjectionFixture::DatabaseError,
    ))
    .await;
}

#[tokio::test]
async fn video_no_writer_preserves_upstream_success_once() {
    for finalize in [false, true] {
        Box::pin(assert_video_projection_preserves_upstream_success(
            finalize,
            VideoProjectionFixture::NoWriter,
        ))
        .await;
    }
}
