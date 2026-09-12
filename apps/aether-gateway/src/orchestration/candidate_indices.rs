use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::GatewayError;

tokio::task_local! {
    static REQUEST_CANDIDATE_INDICES: Arc<RequestCandidateIndices>;
}

/// Candidate indices identify logical observations across all planner steps of
/// one request. Same-key and pool-key retries retain their existing retry index.
#[derive(Debug, Default)]
pub(crate) struct RequestCandidateIndices {
    next: AtomicU64,
}

impl RequestCandidateIndices {
    pub(crate) fn reserve(&self, count: usize) -> Result<u32, GatewayError> {
        let count = u64::try_from(count).unwrap_or(u64::MAX);
        self.next
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                next.checked_add(count)
                    .filter(|end| *end <= i32::MAX as u64 + 1)
            })
            .map(|start| start as u32)
            .map_err(|_| GatewayError::Internal("request candidate index space exhausted".into()))
    }
}

pub(crate) fn current_request_candidate_indices() -> Arc<RequestCandidateIndices> {
    REQUEST_CANDIDATE_INDICES
        .try_with(Arc::clone)
        .unwrap_or_default()
}

pub(crate) async fn scope_request_candidate_indices<F: Future>(future: F) -> F::Output {
    scope_request_candidate_indices_with(Arc::new(RequestCandidateIndices::default()), future).await
}

pub(crate) async fn scope_request_candidate_indices_with<F: Future>(
    indices: Arc<RequestCandidateIndices>,
    future: F,
) -> F::Output {
    REQUEST_CANDIDATE_INDICES.scope(indices, future).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn separate_planners_share_indices_and_new_requests_restart_at_zero() {
        for _ in 0..2 {
            scope_request_candidate_indices(async {
                let first_step = current_request_candidate_indices();
                let later_step = current_request_candidate_indices();
                assert_eq!(first_step.reserve(2).unwrap(), 0);
                assert_eq!(later_step.reserve(1).unwrap(), 2);
                // A lazy cursor keeps its handle when moved to a spawned task.
                assert_eq!(
                    tokio::spawn(async move { first_step.reserve(2).unwrap() })
                        .await
                        .unwrap(),
                    3
                );
                assert_eq!(later_step.reserve(1).unwrap(), 5);
            })
            .await;
        }
    }

    #[test]
    fn exhausted_postgres_index_space_is_never_reused() {
        let indices = RequestCandidateIndices {
            next: AtomicU64::new(i32::MAX as u64),
        };
        assert_eq!(indices.reserve(1).unwrap(), i32::MAX as u32);
        assert!(indices.reserve(1).is_err());
        assert!(indices.reserve(usize::MAX).is_err());
        assert_eq!(indices.next.load(Ordering::Relaxed), i32::MAX as u64 + 1);
    }
}
