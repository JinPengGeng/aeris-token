# Issue 216: require execution of each selected live database test

The live PostgreSQL harness used exact test names but accepted libtest's zero
exit status when a renamed or removed test selected no tests. Each invocation
now retains its combined output, preserves Cargo and log-writer failures, and
requires exactly one summary with one passed, zero failed and zero ignored.
Successful local runs remove their temporary logs; failed runs print the
retained evidence directory. Logs use `AGENT_TMP_DIR`, `RUNNER_TEMP` or the
system temporary directory and remain visible in the hosted job output.

The PostgreSQL, Gateway funds and Redis live runners use `cargo test --locked`
so their dependency resolution stays tied to the committed `Cargo.lock`.

The existing ignored
`video_tasks::tests::live_video_task_capture_claim_and_completion_preserve_business_fields`
is now selected after the migration prerequisite. The required
`data_db_ignored_postgres` job invokes this harness; the ordinary adapter
nextest job does not execute ignored tests.

`tests/postgres_live_test_gate_test.sh` runs the real harness against command
fixtures, covering a successful selection, zero tests, ignored tests, multiple
tests, absent or duplicate summaries, a Cargo failure with a successful-looking
summary, and a log-writer failure. It also verifies the video target is reached
and that a failed target stops subsequent work. Rust CI's required shell
fixtures run this contract without compiling Rust or contacting a database.

These fixture checks prove the execution gate, not video persistence or claim
fencing behavior. Acceptance still requires the current candidate's real
PostgreSQL job to report one passed, zero failed and zero ignored for every
selected test, including the video test. Historical runs of older heads do
not validate these changes.

The recharge follow-up expands the PostgreSQL inventory to 57 exact targets,
including 20 recovery cases and one real JSONL restore/callback regression.
The Gateway harness now applies the same execution and failure-propagation
gate to its 21 image funding/receipt targets plus the real recharge-to-SMTP
target. `tests/gateway_live_test_gate_test.sh` verifies that full inventory,
zero/ignored/multiple or missing summaries, Cargo failure and log-write failure;
Rust CI runs both harness fixtures. These counts describe required selections,
not a claim that a particular candidate has passed them.
