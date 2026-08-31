use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::response::IntoResponse;
use axum::routing::any;
use axum::{Json, Router};

use crate::scheduler_acceptance::NetworkObservation;
use crate::server::SpawnedServer;

#[derive(Debug, Clone)]
struct RecorderState {
    candidate: Arc<str>,
    observation: Arc<Mutex<NetworkObservation>>,
}

#[derive(Debug)]
pub struct CountingUpstreamRecorder {
    server: SpawnedServer,
    state: RecorderState,
}

impl CountingUpstreamRecorder {
    pub async fn start(candidate: impl Into<Arc<str>>) -> Result<Self, String> {
        let state = RecorderState {
            candidate: candidate.into(),
            observation: Arc::new(Mutex::new(NetworkObservation::default())),
        };
        let router = Router::new()
            .fallback(any(record_request))
            .with_state(state.clone());
        let server = SpawnedServer::start(router)
            .await
            .map_err(|error| format!("failed to start counting upstream recorder: {error}"))?;
        Ok(Self { server, state })
    }

    pub fn base_url(&self) -> &str {
        self.server.base_url()
    }

    pub fn snapshot(&self) -> NetworkObservation {
        self.state
            .observation
            .lock()
            .expect("upstream recorder mutex poisoned")
            .clone()
    }
}

async fn record_request(
    State(state): State<RecorderState>,
    request: Request<Body>,
) -> impl IntoResponse {
    let request_id = header(&request, "x-aether-acceptance-request")
        .unwrap_or_else(|| format!("unidentified-{}", state.candidate));
    let attempt = header(&request, "x-aether-acceptance-attempt")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let credential = header(&request, "x-aether-acceptance-credential");
    state
        .observation
        .lock()
        .expect("upstream recorder mutex poisoned")
        .record(request_id, attempt, state.candidate.to_string(), credential);
    Json(serde_json::json!({
        "id": "issue47-recorder-response",
        "object": "chat.completion",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}}]
    }))
}

fn header(request: &Request<Body>, name: &str) -> Option<String> {
    request
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}
