use super::*;
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::video_tasks::{InMemoryVideoTaskRepository, SqlxVideoTaskRepository};
use aether_data_contracts::repository::candidates::{
    RequestCandidateReadRepository, RequestCandidateStatus, StoredRequestCandidate,
};
use aether_data_contracts::repository::video_tasks::{
    UpsertVideoTask, VideoTaskStatus, VideoTaskWriteRepository,
};
use std::sync::Arc;
use std::time::Duration;

async fn assert_internal_video_projection_failure_still_reports_success(
    finalize: bool,
    database_error: bool,
) {
    let candidate = StoredRequestCandidate::new(
        "internal-video-candidate".to_string(),
        "internal-video-request".to_string(),
        Some("video-user".to_string()),
        Some("video-api-key".to_string()),
        Some("alice".to_string()),
        Some("default".to_string()),
        0,
        0,
        Some("video-provider".to_string()),
        Some("video-endpoint".to_string()),
        Some("video-key".to_string()),
        RequestCandidateStatus::Pending,
        None,
        false,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        1_700_000_000_000,
        Some(1_700_000_000_000),
        None,
    )
    .unwrap();
    let candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![candidate]));
    let tasks = Arc::new(InMemoryVideoTaskRepository::default());
    let data = crate::data::GatewayDataState::with_request_candidate_repository_for_tests(
        Arc::clone(&candidates),
    );
    let data = if database_error {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://fixture:fixture@127.0.0.1:1/unused")
            .unwrap();
        pool.close().await;
        data.attach_video_task_repository_for_tests(Arc::new(SqlxVideoTaskRepository::new(pool)))
    } else {
        data.attach_video_task_repository_for_tests(Arc::clone(&tasks))
    };
    let state = AppState::new()
        .unwrap()
        .with_video_task_truth_source_mode(
            crate::video_tasks::VideoTaskTruthSourceMode::RustAuthoritative,
        )
        .with_data_state_for_tests(data);
    let provider_body = json!({"id":"internal-upstream-video", "status":"queued"});
    let mut context = json!({
        "request_id":"internal-video-request", "candidate_id":"internal-video-candidate",
        "user_id":"video-user", "api_key_id":"video-api-key",
        "provider_id":"video-provider", "endpoint_id":"video-endpoint", "key_id":"video-key",
        "client_api_format":"openai:video", "provider_api_format":"openai:video",
        "model":"sora-2", "mapped_model":"sora-2",
        "local_task_id":"internal-local-video", "task_id":"internal-local-video",
        "local_created_at":1_700_000_000,
        "original_request_body":{"model":"sora-2", "prompt":"fixture"}
    });
    let plan =
        build_internal_finalize_video_plan("internal-video-trace", "openai:video", Some(&context))
            .unwrap();
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
        let observed = seed.with_stored_task(&stored).unwrap();
        context["video_task_row_revision"] = json!(observed.row_revision());
        state.video_tasks.record_snapshot(observed);
    }
    if !database_error {
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
    let payload = crate::usage::GatewaySyncReportRequest {
        trace_id: "internal-video-trace".to_string(),
        report_kind: if finalize {
            "openai_video_cancel_sync_finalize"
        } else {
            "openai_video_create_sync_finalize"
        }
        .to_string(),
        report_context: Some(context),
        status_code: 200,
        headers: BTreeMap::new(),
        body_json: Some(if finalize { json!({}) } else { provider_body }),
        client_body_json: None,
        body_base64: None,
        telemetry: None,
    };
    let decision = build_internal_finalize_decision(&payload).unwrap();
    let result = Box::pin(maybe_build_internal_finalize_video_response(
        &state,
        "internal-video-trace",
        &decision,
        payload,
    ))
    .await;
    if database_error {
        assert!(matches!(result, Err(GatewayError::Internal(_))));
    } else {
        assert!(matches!(
            result,
            Err(GatewayError::Client {
                status: http::StatusCode::CONFLICT,
                ..
            })
        ));
    }
    // Finalize reports are spawned; wait for their observable candidate update.
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let recorded = candidates
                .list_by_request_id("internal-video-request")
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
    .expect("upstream success report must survive a projection failure");
}

#[tokio::test]
async fn internal_video_create_conflict_still_reports_success() {
    Box::pin(assert_internal_video_projection_failure_still_reports_success(false, false)).await;
}

#[tokio::test]
async fn internal_video_create_database_error_still_reports_success() {
    Box::pin(assert_internal_video_projection_failure_still_reports_success(false, true)).await;
}

#[tokio::test]
async fn internal_video_finalize_conflict_still_reports_success() {
    Box::pin(assert_internal_video_projection_failure_still_reports_success(true, false)).await;
}

#[tokio::test]
async fn internal_video_finalize_database_error_still_reports_success() {
    Box::pin(assert_internal_video_projection_failure_still_reports_success(true, true)).await;
}
