use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::{Body, Bytes};
use axum::http::Response;
use http_body::{Body as HttpBody, Frame, SizeHint};

const PREPARED: u8 = 0;
const SENT_BUT_UNCOMMITTED: u8 = 1;
const CLIENT_COMMITTED: u8 = 2;
const TERMINAL: u8 = 3;

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttemptLifecyclePhase {
    Prepared,
    SentButUncommitted,
    ClientCommitted,
    Terminal,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RequestCommitBarrier(Arc<AtomicBool>);

impl RequestCommitBarrier {
    pub(crate) fn is_client_committed(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    fn mark_client_committed(&self) {
        self.0.store(true, Ordering::Release);
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AttemptLifecycle {
    phase: Arc<AtomicU8>,
    request_commit_barrier: RequestCommitBarrier,
}

impl AttemptLifecycle {
    pub(crate) fn new(request_commit_barrier: RequestCommitBarrier) -> Self {
        Self {
            phase: Arc::new(AtomicU8::new(PREPARED)),
            request_commit_barrier,
        }
    }

    pub(crate) fn mark_sent(&self) {
        let _ = self.phase.compare_exchange(
            PREPARED,
            SENT_BUT_UNCOMMITTED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    pub(crate) fn mark_client_committed(&self) {
        if self.phase.load(Ordering::Acquire) == SENT_BUT_UNCOMMITTED {
            self.request_commit_barrier.mark_client_committed();
            self.phase.store(CLIENT_COMMITTED, Ordering::Release);
        }
    }

    pub(crate) fn mark_terminal(&self) {
        self.phase.store(TERMINAL, Ordering::Release);
    }

    pub(crate) fn into_terminal_guard(self) -> AttemptLifecycleTerminalGuard {
        AttemptLifecycleTerminalGuard { lifecycle: self }
    }

    fn is_terminal(&self) -> bool {
        self.phase.load(Ordering::Acquire) == TERMINAL
    }

    #[cfg(test)]
    pub(crate) fn phase(&self) -> AttemptLifecyclePhase {
        match self.phase.load(Ordering::Acquire) {
            PREPARED => AttemptLifecyclePhase::Prepared,
            SENT_BUT_UNCOMMITTED => AttemptLifecyclePhase::SentButUncommitted,
            CLIENT_COMMITTED => AttemptLifecyclePhase::ClientCommitted,
            TERMINAL => AttemptLifecyclePhase::Terminal,
            _ => unreachable!("attempt lifecycle phase must be valid"),
        }
    }
}

/// Owns a non-HTTP attempt lifecycle until the WS turn is explicitly ended.
/// Dropping the guard is a cancellation fallback; it never commits a request.
#[derive(Debug)]
pub(crate) struct AttemptLifecycleTerminalGuard {
    lifecycle: AttemptLifecycle,
}

impl AttemptLifecycleTerminalGuard {
    pub(crate) fn mark_sent(&self) {
        self.lifecycle.mark_sent();
    }

    pub(crate) fn mark_client_committed(&self) {
        self.lifecycle.mark_client_committed();
    }

    pub(crate) fn mark_terminal(&self) {
        self.lifecycle.mark_terminal();
    }

    #[cfg(test)]
    pub(crate) fn phase(&self) -> AttemptLifecyclePhase {
        self.lifecycle.phase()
    }
}

impl Drop for AttemptLifecycleTerminalGuard {
    fn drop(&mut self) {
        self.lifecycle.mark_terminal();
    }
}

struct LifecycleBody {
    body: Body,
    lifecycle: AttemptLifecycle,
}

impl HttpBody for LifecycleBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.body).poll_frame(context);
        match &result {
            Poll::Ready(Some(Ok(frame))) => {
                if frame.data_ref().is_some_and(|bytes| !bytes.is_empty()) {
                    this.lifecycle.mark_client_committed();
                }
            }
            Poll::Ready(Some(Err(_))) => {
                this.lifecycle.mark_terminal();
            }
            Poll::Ready(None) => {
                this.lifecycle.mark_client_committed();
                this.lifecycle.mark_terminal();
            }
            Poll::Pending => {}
        }
        result
    }

    fn is_end_stream(&self) -> bool {
        // Returning false until poll_frame observes EOF makes an otherwise
        // empty response pass through the same completion handoff as data.
        // Construction or a pre-delivery drop must not commit the request.
        self.lifecycle.is_terminal() && self.body.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}

impl Drop for LifecycleBody {
    fn drop(&mut self) {
        self.lifecycle.mark_terminal();
    }
}

/// This wrapper is installed only after an execution runtime has returned its
/// final HTTP response. The first non-empty data item is the gateway's
/// application-layer client handoff, not a socket ACK.
pub(crate) fn wrap_response_body_with_attempt_lifecycle(
    response: Response<Body>,
    lifecycle: AttemptLifecycle,
) -> Response<Body> {
    let (parts, body) = response.into_parts();
    let body = LifecycleBody { body, lifecycle };
    Response::from_parts(parts, Body::new(body))
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::pin::Pin;

    use axum::body::{Body, Bytes};
    use axum::http::{header, HeaderMap, HeaderValue, Response, StatusCode};
    use futures_util::StreamExt;
    use http_body::{Body as HttpBody, Frame};

    use super::{
        wrap_response_body_with_attempt_lifecycle, AttemptLifecycle, AttemptLifecyclePhase,
        RequestCommitBarrier,
    };

    #[tokio::test]
    async fn first_body_yield_commits_without_changing_headers_or_payload() {
        let barrier = RequestCommitBarrier::default();
        let lifecycle = AttemptLifecycle::new(barrier.clone());
        lifecycle.mark_sent();
        let response = Response::builder()
            .status(StatusCode::CREATED)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{\"ok\":true}"))
            .expect("response should build");
        let response = wrap_response_body_with_attempt_lifecycle(response, lifecycle.clone());

        assert!(!barrier.is_client_committed());
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("wrapped body should read");
        assert_eq!(bytes.as_ref(), b"{\"ok\":true}");
        assert!(barrier.is_client_committed());
        assert_eq!(lifecycle.phase(), AttemptLifecyclePhase::Terminal);
    }

    #[tokio::test]
    async fn constructed_but_unpolled_body_stays_precommit() {
        let barrier = RequestCommitBarrier::default();
        let lifecycle = AttemptLifecycle::new(barrier.clone());
        lifecycle.mark_sent();
        let response = wrap_response_body_with_attempt_lifecycle(
            Response::new(Body::from("not yet yielded")),
            lifecycle.clone(),
        );

        assert!(!barrier.is_client_committed());
        assert_eq!(lifecycle.phase(), AttemptLifecyclePhase::SentButUncommitted);
        drop(response);
        assert!(!barrier.is_client_committed());
        assert_eq!(lifecycle.phase(), AttemptLifecyclePhase::Terminal);
    }

    #[tokio::test]
    async fn drop_after_first_yield_keeps_request_barrier_committed() {
        let barrier = RequestCommitBarrier::default();
        let lifecycle = AttemptLifecycle::new(barrier.clone());
        lifecycle.mark_sent();
        let response = wrap_response_body_with_attempt_lifecycle(
            Response::new(Body::from("first chunk")),
            lifecycle.clone(),
        );
        let mut stream = response.into_body().into_data_stream();

        let first = stream
            .next()
            .await
            .expect("first item")
            .expect("body item should succeed");
        assert_eq!(first.as_ref(), b"first chunk");
        assert!(barrier.is_client_committed());
        drop(stream);
        assert!(barrier.is_client_committed());
        assert_eq!(lifecycle.phase(), AttemptLifecyclePhase::Terminal);
    }

    #[tokio::test]
    async fn read_error_before_first_frame_stays_uncommitted() {
        let barrier = RequestCommitBarrier::default();
        let lifecycle = AttemptLifecycle::new(barrier.clone());
        lifecycle.mark_sent();
        let response = wrap_response_body_with_attempt_lifecycle(
            Response::new(Body::from_stream(futures_util::stream::iter([Err::<
                axum::body::Bytes,
                std::io::Error,
            >(
                std::io::Error::other("upstream read failed"),
            )]))),
            lifecycle.clone(),
        );

        assert!(axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .is_err());
        assert!(!barrier.is_client_committed());
        assert_eq!(lifecycle.phase(), AttemptLifecyclePhase::Terminal);
    }

    #[tokio::test]
    async fn empty_body_eof_commits_the_completed_response() {
        let barrier = RequestCommitBarrier::default();
        let lifecycle = AttemptLifecycle::new(barrier.clone());
        lifecycle.mark_sent();
        let response = wrap_response_body_with_attempt_lifecycle(
            Response::new(Body::empty()),
            lifecycle.clone(),
        );

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("empty response should read");
        assert!(bytes.is_empty());
        assert!(barrier.is_client_committed());
        assert_eq!(lifecycle.phase(), AttemptLifecyclePhase::Terminal);
    }

    #[test]
    fn terminal_guard_marks_a_dropped_ws_attempt_terminal_without_committing() {
        let barrier = RequestCommitBarrier::default();
        let lifecycle = AttemptLifecycle::new(barrier.clone());
        let observed = lifecycle.clone();
        let guard = lifecycle.into_terminal_guard();
        guard.mark_sent();

        drop(guard);

        assert!(!barrier.is_client_committed());
        assert_eq!(observed.phase(), AttemptLifecyclePhase::Terminal);
    }

    #[tokio::test]
    async fn frame_wrapper_preserves_trailers() {
        let barrier = RequestCommitBarrier::default();
        let lifecycle = AttemptLifecycle::new(barrier);
        lifecycle.mark_sent();
        let mut trailers = HeaderMap::new();
        trailers.insert("x-upstream-trailer", HeaderValue::from_static("retained"));
        let source = http_body_util::StreamBody::new(futures_util::stream::iter([
            Ok::<_, Infallible>(Frame::data(Bytes::from_static(b"data"))),
            Ok(Frame::trailers(trailers)),
        ]));
        let response =
            wrap_response_body_with_attempt_lifecycle(Response::new(Body::new(source)), lifecycle);
        let mut body = response.into_body();

        let data = std::future::poll_fn(|context| Pin::new(&mut body).poll_frame(context))
            .await
            .expect("data frame")
            .expect("data should succeed")
            .into_data()
            .expect("first frame remains data");
        assert_eq!(data.as_ref(), b"data");
        let trailers = std::future::poll_fn(|context| Pin::new(&mut body).poll_frame(context))
            .await
            .expect("trailers frame")
            .expect("trailers should succeed")
            .into_trailers()
            .expect("second frame remains trailers");
        assert_eq!(trailers["x-upstream-trailer"], "retained");
    }
}
