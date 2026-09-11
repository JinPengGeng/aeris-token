# TaskSupervisor drop lifecycle decision (#211)

## Current finding

`TaskSupervisor` stores wrapper tasks in a `JoinSet`. Each wrapper owns a second
`JoinHandle` for the actual task so that completion, panic, abort, and cancellation
can be classified in metrics. Previously, dropping the supervisor aborted only the
wrappers. Dropping an inner `JoinHandle` detaches its task, so both `spawn_named`
and `supervise_handle` tasks could continue without an owner.

## Decision

`Drop` now cancels the supervisor token and detaches the wrappers from the
`JoinSet`. The already-running wrappers observe cancellation, abort and reap their
inner handles, and record the existing `cancelled` metric. This preserves explicit
`shutdown()` semantics and the existing panic/abort classification while avoiding
a second task registry or synchronous blocking in `Drop`.

Detaching is limited to the cancellation wrappers themselves. They have no
long-running work after cancellation: each aborts its inner task, awaits the abort,
records cancellation, and exits. If the Tokio runtime itself is shutting down, the
runtime remains the final owner and drops both wrapper and inner futures.

## Regression coverage

`dropping_supervisor_cancels_spawned_and_supervised_tasks` covers both entry
points. It waits until both real futures have started, drops the supervisor, then
verifies that both futures are dropped, their progress counters stop changing, and
metrics settle at two cancellations with no completion, panic, or external-abort
classification.
