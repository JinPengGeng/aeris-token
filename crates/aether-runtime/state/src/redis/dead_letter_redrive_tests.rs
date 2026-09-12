use super::*;

/// Exercise the public runtime redrive API against an actual Redis server. The
/// lower-level transfer tests cover pending-entry archival; this test covers
/// the operator-facing stream lookup/redrive path and its retention bound.
#[tokio::test]
async fn redis_dead_letter_redrive_is_idempotent_and_retention_bounded() {
    let Some((_server, runtime)) = redis_runtime_for_test("dlq-redrive").await else {
        eprintln!("DLQ redrive test skipped: isolated Redis fixture unavailable");
        return;
    };

    let source = "usage:dlq:redrive-source";
    let destination = "usage:dlq:redrive-destination";
    let destination_maxlen = 64;
    let seeded_entries = destination_maxlen * 4;
    let mut source_ids = Vec::new();
    for index in 0..seeded_entries {
        let fields = BTreeMap::from([
            ("payload".to_string(), format!("dead-letter-{index}")),
            ("error".to_string(), "provider rejected request".to_string()),
        ]);
        source_ids.push(
            RuntimeQueueStore::append_fields_with_maxlen(&runtime, source, &fields, None)
                .await
                .expect("seed DLQ entry"),
        );
    }

    let replay_fields = BTreeMap::from([
        ("payload".to_string(), "replayed-event".to_string()),
        ("source".to_string(), "operator-redrive-test".to_string()),
    ]);
    let first = RuntimeQueueStore::redrive_stream_entry(
        &runtime,
        source,
        &source_ids[0],
        destination,
        &replay_fields,
        Some(destination_maxlen),
    )
    .await
    .expect("first redrive");
    let destination_id = match first {
        RuntimeQueueRedriveOutcome::Redriven { destination_id } => destination_id,
        other => panic!("unexpected first redrive outcome: {other:?}"),
    };

    let retry = RuntimeQueueStore::redrive_stream_entry(
        &runtime,
        source,
        &source_ids[0],
        destination,
        &replay_fields,
        Some(destination_maxlen),
    )
    .await
    .expect("idempotent redrive retry");
    assert_eq!(
        retry,
        RuntimeQueueRedriveOutcome::AlreadyRedriven {
            destination_id: destination_id.clone()
        }
    );

    for source_id in &source_ids[1..] {
        assert!(matches!(
            RuntimeQueueStore::redrive_stream_entry(
                &runtime,
                source,
                source_id,
                destination,
                &replay_fields,
                Some(destination_maxlen),
            )
            .await
            .expect("redrive remaining DLQ entry"),
            RuntimeQueueRedriveOutcome::Redriven { .. }
        ));
    }

    let source_stats = RuntimeQueueStore::stats(&runtime, source, None)
        .await
        .expect("source stats");
    let destination_stats = RuntimeQueueStore::stats(&runtime, destination, None)
        .await
        .expect("destination stats");
    assert_eq!(source_stats.stream_length, 0);
    // Redis uses approximate MAXLEN trimming for throughput.  The stream must
    // still stay within one radix-tree node of the configured cap rather than
    // growing with every redrive.
    assert!(
        destination_stats.stream_length <= (destination_maxlen + 100) as u64,
        "destination retention grew beyond bounded MAXLEN slack: {:?}",
        destination_stats
    );
    let destination_page = RuntimeQueueStore::read_stream_page(
        &runtime,
        destination,
        "0-0",
        destination_maxlen + 1,
    )
    .await
    .expect("destination page");
    assert!(destination_page.entries.iter().all(|entry| {
        entry
            .fields
            .get("source")
            .is_some_and(|value| value == "operator-redrive-test")
    }));
}
