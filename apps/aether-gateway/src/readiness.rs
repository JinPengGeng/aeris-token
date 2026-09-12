//! Readiness is an admission signal, independent from the process liveness route.
use std::future::Future;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::FutureExt;
use serde::Serialize;
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::AppState;

pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(1);
pub(crate) const CACHE_TTL: Duration = Duration::from_secs(1);
const STARTING: u8 = 0;
const RUNNING: u8 = 1;
const CLOSING: u8 = 2;
const USAGE_WORKER: u8 = 1;
const COUNTER_WORKER: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CheckStatus {
    Disabled,
    Unchecked,
    Ok,
    Failed,
    Timeout,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct Check {
    status: CheckStatus,
    required: bool,
}

impl Check {
    fn new(required: bool, status: CheckStatus) -> Self {
        Self {
            status: if required {
                status
            } else {
                CheckStatus::Disabled
            },
            required,
        }
    }

    fn ready(self) -> bool {
        !self.required || self.status == CheckStatus::Ok
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct Dependencies {
    database: Check,
    redis: Check,
}

impl Dependencies {
    fn unchecked(database: bool, redis: bool) -> Self {
        Self {
            database: Check::new(database, CheckStatus::Unchecked),
            redis: Check::new(redis, CheckStatus::Unchecked),
        }
    }

    fn ready(self) -> bool {
        self.database.ready() && self.redis.ready()
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct Workers {
    usage_queue: Check,
    usage_counter_flush: Check,
}

impl Workers {
    fn ready(self) -> bool {
        self.usage_queue.ready() && self.usage_counter_flush.ready()
    }
}

#[derive(Debug, Default)]
struct ProbeState {
    cached: Option<(Instant, Dependencies)>,
    flight: Option<watch::Receiver<Option<Dependencies>>>,
}

#[derive(Debug, Default)]
pub(crate) struct Readiness {
    phase: AtomicU8,
    required_workers: AtomicU8,
    closing: CancellationToken,
    probes: Arc<Mutex<ProbeState>>,
}

impl Readiness {
    fn phase(&self) -> &'static str {
        match self.phase.load(Ordering::Acquire) {
            RUNNING => "running",
            CLOSING => "closing",
            _ => "starting",
        }
    }

    // Keep the operation alive when its initiating HTTP client disconnects. There is
    // at most one pair of probes, sharing one deadline, even during an outage storm.
    async fn probe<F, Fut>(&self, probe: F) -> Option<Dependencies>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Dependencies> + Send + 'static,
    {
        let mut flight = {
            let mut state = self.probes.lock().unwrap_or_else(|err| err.into_inner());
            if let Some((expires, snapshot)) = state.cached {
                if Instant::now() < expires {
                    return Some(snapshot);
                }
            }
            if let Some(flight) = &state.flight {
                flight.clone()
            } else {
                let (sender, receiver) = watch::channel(None);
                state.flight = Some(receiver.clone());
                let probes = self.probes.clone();
                tokio::spawn(async move {
                    // A panicking probe must release the flight and fail closed,
                    // rather than leave every subsequent request waiting forever.
                    let snapshot = std::panic::AssertUnwindSafe(probe())
                        .catch_unwind()
                        .await
                        .ok();
                    let mut state = probes.lock().unwrap_or_else(|err| err.into_inner());
                    if let Some(snapshot) = snapshot {
                        state.cached = Some((Instant::now() + CACHE_TTL, snapshot));
                        sender.send_replace(Some(snapshot));
                    }
                    state.flight = None;
                });
                receiver
            }
        };
        tokio::select! {
            biased;
            _ = self.closing.cancelled() => None,
            // The producer owns the one overall deadline. A second racing timer
            // here could discard an already-completed DB result when Redis times out.
            result = flight.wait_for(|value| value.is_some()) => {
                result.ok().and_then(|value| *value)
            }
        }
    }
}

async fn dependency_probe<F, E>(required: bool, deadline: Instant, probe: F) -> Check
where
    F: Future<Output = Result<(), E>>,
{
    if !required {
        return Check::new(false, CheckStatus::Disabled);
    }
    // timeout_at polls a ready future before checking its timer. Require completion
    // strictly before the deadline so boundary results cannot incorrectly pass.
    let status = match tokio::time::timeout_at(deadline, probe).await {
        _ if Instant::now() >= deadline => CheckStatus::Timeout,
        Ok(Ok(())) => CheckStatus::Ok,
        Ok(Err(_)) => CheckStatus::Failed,
        Err(_) => CheckStatus::Timeout,
    };
    Check::new(true, status)
}

impl AppState {
    /// Call only after startup preparation and the selected role's worker registration.
    /// A closed gateway cannot be reopened by a late startup completion.
    pub fn mark_startup_complete(&self, background_workers_enabled: bool) {
        let mut required = 0;
        if background_workers_enabled {
            if self
                .usage_runtime
                .can_spawn_worker(self.background_data.as_ref())
            {
                required |= USAGE_WORKER;
            }
            if self.background_data.has_usage_counter_flush_backend() {
                required |= COUNTER_WORKER;
            }
        }
        self.readiness
            .required_workers
            .store(required, Ordering::Release);
        let _ = self.readiness.phase.compare_exchange(
            STARTING,
            RUNNING,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    /// Withdraw immediately; the server may continue draining existing requests.
    pub fn begin_readiness_shutdown(&self) {
        self.readiness.phase.store(CLOSING, Ordering::Release);
        self.readiness.closing.cancel();
    }

    fn readiness_workers(&self) -> Workers {
        let required = self.readiness.required_workers.load(Ordering::Acquire);
        let tasks = self.task_supervisor_metrics.snapshot();
        let running = |key| {
            tasks
                .tasks
                .iter()
                .any(|task| task.task_name == key && task.active_tasks > 0)
        };
        let usage = self.usage_runtime.metrics_snapshot();
        Workers {
            usage_queue: Check::new(
                required & USAGE_WORKER != 0,
                if running(crate::task_runtime::TASK_KEY_USAGE_QUEUE_WORKER)
                    && usage.worker_active_count > 0
                    && !usage.shutdown_started
                {
                    CheckStatus::Ok
                } else {
                    CheckStatus::Failed
                },
            ),
            usage_counter_flush: Check::new(
                required & COUNTER_WORKER != 0,
                if running(crate::task_runtime::TASK_KEY_USAGE_COUNTER_FLUSH) {
                    CheckStatus::Ok
                } else {
                    CheckStatus::Failed
                },
            ),
        }
    }

    pub(crate) async fn readiness_snapshot(&self) -> ReadinessSnapshot {
        let deadline = Instant::now() + PROBE_TIMEOUT;
        let database = self.has_data_backends();
        let redis = self.has_redis_data_backend();
        let mut dependencies = Dependencies::unchecked(database, redis);
        if self.readiness.phase() == "running" && self.readiness_workers().ready() {
            let state = self.clone();
            dependencies = self
                .readiness
                .probe(move || async move {
                    let (database, redis) = tokio::join!(
                        dependency_probe(database, deadline, state.data.ping_database()),
                        dependency_probe(redis, deadline, state.ping_runtime_state()),
                    );
                    Dependencies { database, redis }
                })
                .await
                .unwrap_or(Dependencies {
                    database: Check::new(database, CheckStatus::Timeout),
                    redis: Check::new(redis, CheckStatus::Timeout),
                });
        }
        // Lifecycle and worker health are never cached. Recheck after awaited I/O
        // so a successful old probe cannot reopen readiness during shutdown.
        let lifecycle_status = self.readiness.phase();
        let workers = self.readiness_workers();
        ReadinessSnapshot {
            ready: lifecycle_status == "running" && dependencies.ready() && workers.ready(),
            lifecycle_status,
            dependencies,
            workers,
        }
    }
}

pub(crate) struct ReadinessSnapshot {
    pub(crate) ready: bool,
    pub(crate) lifecycle_status: &'static str,
    pub(crate) dependencies: Dependencies,
    pub(crate) workers: Workers,
}

#[cfg(test)]
mod tests;
