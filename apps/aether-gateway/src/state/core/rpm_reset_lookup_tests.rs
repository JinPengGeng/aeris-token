use super::{AppState, PROVIDER_KEY_RPM_WINDOW_SECS};

#[test]
fn provider_rpm_reset_preserves_inclusive_window_and_timestamp_boundaries() {
    let state = AppState::new().expect("state should build");
    state.mark_provider_key_rpm_reset("key", 100);
    assert_eq!(state.provider_key_rpm_reset_at("key", 99), Some(100));
    assert_eq!(state.provider_key_rpm_reset_at("key", 160), Some(100));
    assert_eq!(state.provider_key_rpm_reset_at("key", 161), None);
    assert_eq!(state.provider_key_rpm_reset_at("key", 160), None);

    state.mark_provider_key_rpm_reset("epoch", 0);
    assert_eq!(state.provider_key_rpm_reset_at("epoch", 59), Some(0));
    assert_eq!(state.provider_key_rpm_reset_at("epoch", 60), Some(0));
    assert_eq!(state.provider_key_rpm_reset_at("epoch", 61), None);
    state.mark_provider_key_rpm_reset("future", u64::MAX);
    assert_eq!(state.provider_key_rpm_reset_at("future", 0), Some(u64::MAX));
    assert_eq!(
        state.provider_key_rpm_reset_at("future", u64::MAX),
        Some(u64::MAX)
    );
}

#[test]
fn provider_rpm_reset_keeps_keys_isolated_and_amortizes_idle_expiration() {
    let state = AppState::new().expect("state should build");
    state.mark_provider_key_rpm_reset("expired", 100);
    state.mark_provider_key_rpm_reset("idle", 100);
    state.mark_provider_key_rpm_reset("fresh", 130);
    assert_eq!(state.provider_key_rpm_reset_at("missing", 161), None);
    assert_eq!(state.provider_key_rpm_reset_at("expired", 161), None);
    assert_eq!(state.provider_key_rpm_reset_at("fresh", 161), Some(130));
    // Hot-path writes stay O(1): below the sweep high-water mark, expired
    // markers of idle keys remain physically stored. Reads lazy-expire per
    // key and never sweep unrelated entries.
    assert!(state.provider_key_rpm_resets.contains_key("idle"));
    state.mark_provider_key_rpm_reset("fresh", 161);
    assert!(state.provider_key_rpm_resets.contains_key("idle"));
    assert_eq!(state.provider_key_rpm_reset_at("fresh", 161), Some(161));
    // Preserve last-write-wins even if the caller's wall clock moves back.
    state.mark_provider_key_rpm_reset("fresh", 155);
    assert_eq!(state.provider_key_rpm_reset_at("fresh", 161), Some(155));
}

#[test]
fn provider_rpm_reset_sweep_reclaims_expired_markers_past_high_water_mark() {
    let state = AppState::new().expect("state should build");
    // Fill past the sweep high-water mark; every marker is expired at now=161.
    for key in 0..AppState::PROVIDER_KEY_RPM_RESET_SWEEP_MIN_LEN {
        state.mark_provider_key_rpm_reset(&format!("idle-{key}"), 100);
    }
    assert_eq!(state.provider_key_rpm_resets.len(), 1024);
    state.mark_provider_key_rpm_reset("active", 161);
    // The write past the high-water mark pays one amortized O(n) sweep.
    assert_eq!(state.provider_key_rpm_resets.len(), 1);
    assert_eq!(state.provider_key_rpm_reset_at("active", 161), Some(161));
    assert_eq!(state.provider_key_rpm_reset_at("idle-0", 161), None);
}

#[test]
fn provider_rpm_reset_expiring_read_cannot_remove_a_concurrent_refresh() {
    let state = AppState::new().expect("state should build");
    let barrier = std::sync::Barrier::new(2);
    let (refresh_observations, read_observations) = std::thread::scope(|scope| {
        let writer = scope.spawn(|| {
            let mut observations = Vec::with_capacity(100);
            for _ in 0..100 {
                state.mark_provider_key_rpm_reset("key", 100);
                barrier.wait();
                state.mark_provider_key_rpm_reset("key", 200);
                barrier.wait();
                observations.push(state.provider_key_rpm_reset_at("key", 200));
                barrier.wait();
            }
            observations
        });
        let mut observations = Vec::with_capacity(100);
        for _ in 0..100 {
            barrier.wait();
            let observed = state.provider_key_rpm_reset_at("key", 161);
            barrier.wait();
            observations.push((observed, state.provider_key_rpm_reset_at("key", 200)));
            barrier.wait();
        }
        (
            writer.join().expect("refresh thread should finish"),
            observations,
        )
    });
    // Assert only after both threads have completed every barrier. A detected
    // regression must fail the test instead of stranding the other thread.
    assert!(refresh_observations.iter().all(|value| *value == Some(200)));
    for (before_refresh, after_refresh) in read_observations {
        assert!(before_refresh.is_none() || before_refresh == Some(200));
        assert_eq!(after_refresh, Some(200));
    }
}

// Exact lookup body from main before #414, retained only for manual A/B
// measurement. No timing threshold is part of CI acceptance.
fn legacy_lookup(state: &AppState, key_id: &str, now: u64) -> Option<u64> {
    let mut resets: std::collections::HashMap<String, u64> = state
        .provider_key_rpm_resets
        .iter()
        .map(|entry| (entry.key().clone(), *entry.value()))
        .collect();
    let min_kept = now.saturating_sub(PROVIDER_KEY_RPM_WINDOW_SECS);
    resets.retain(|_, reset_at| *reset_at >= min_kept);
    resets.get(key_id).copied()
}

#[test]
#[ignore = "manual bounded RPM reset lookup comparison; no timing gate"]
fn provider_rpm_reset_lookup_microbenchmark() {
    use std::hint::black_box;
    use std::sync::Barrier;
    use std::time::Instant;

    type Lookup = fn(&AppState, &str, u64) -> Option<u64>;

    fn measure(
        state: &AppState,
        keys: &[String],
        workers: usize,
        batches: usize,
        lookup: Lookup,
    ) -> u128 {
        let ready = Barrier::new(workers + 1);
        let start = Barrier::new(workers + 1);
        let done = Barrier::new(workers + 1);
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| {
                    for key in keys {
                        black_box(lookup(state, black_box(key), 100));
                    }
                    ready.wait();
                    start.wait();
                    for _ in 0..batches {
                        for key in keys {
                            black_box(lookup(state, black_box(key), 100));
                        }
                    }
                    done.wait();
                });
            }
            ready.wait();
            let started = Instant::now();
            start.wait();
            done.wait();
            started.elapsed().as_nanos()
        })
    }

    println!("scenario,capacity,candidates,workers,queries,legacy_median_ns,lookup_median_ns");
    for (scenario, count, keep_one) in [
        ("empty", 0, false),
        ("one", 1, false),
        ("64", 64, false),
        ("4096", 4096, false),
        ("high_water_one", 4096, true),
    ] {
        let state = AppState::new().expect("benchmark state should build");
        let capacity = {
            for key in 0..count {
                state
                    .provider_key_rpm_resets
                    .insert(format!("key-{key}"), 100);
            }
            if keep_one {
                state
                    .provider_key_rpm_resets
                    .retain(|key, _| key.as_str() == "key-0");
            }
            state.provider_key_rpm_resets.len()
        };
        let active = if keep_one { 1 } else { count.max(1) };
        let batches = if count > 64 { 8 } else { 128 };
        for candidates in [1, 32, 128] {
            let keys: Vec<_> = (0..candidates)
                .map(|key| format!("key-{}", key % active))
                .collect();
            for workers in [1, 8] {
                let mut legacy = Vec::new();
                let mut current = Vec::new();
                for round in 0..5 {
                    let methods: [(bool, Lookup); 2] = if round % 2 == 0 {
                        [
                            (true, legacy_lookup),
                            (false, AppState::provider_key_rpm_reset_at),
                        ]
                    } else {
                        [
                            (false, AppState::provider_key_rpm_reset_at),
                            (true, legacy_lookup),
                        ]
                    };
                    for (old, lookup) in methods {
                        let elapsed = measure(&state, &keys, workers, batches, lookup);
                        if old {
                            legacy.push(elapsed);
                        } else {
                            current.push(elapsed);
                        }
                    }
                }
                legacy.sort_unstable();
                current.sort_unstable();
                println!(
                    "{scenario},{capacity},{candidates},{workers},{},{},{}",
                    candidates * workers * batches,
                    legacy[2],
                    current[2]
                );
            }
        }
    }
}
