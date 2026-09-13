# Issue #205: admission covers the full tunneled stream

Decision: implement. Priority P1, complexity S-M. Parent #205 remains open;
this slice also supplies part of #214's capacity-isolation evidence. Fork only.

## Accuracy and benefit

`max_in_flight_streams` and the distributed semaphore describe complete in-flight
tunneled streams. Three terminal response paths previously dropped the combined
admission permit before `relay_upstream_response` read the body. A slow upstream
could therefore send headers, free its admission slot and keep a connection and
response buffers occupied while further streams were admitted. Active-stream
metrics already covered the whole body, so admission and metrics disagreed.

## Implementation

Retain the existing RAII permit in `handle_stream_inner` until the whole handler
returns or is cancelled. The same guard now spans successful redirect chains,
preserved redirects whose body cannot be replayed, response flow-control waits,
body timeout/error and successful EOF. The underlying distributed permit already
owns lease renewal and release; no new lease or semaphore implementation is added.
Early request failures release through the same normal guard drop.

The practical behavior change is intentional: a one-slot tunnel admits one active
response body at a time. Operators who relied on header-only occupancy may see
overload responses sooner and must size stream limits for actual streaming load.

## Acceptance

- Real loopback HTTP responses send headers and block their bodies on an explicit
  signal; both local and distributed-memory admission must remain saturated.
- A second handler receives the existing `tunnel overloaded` response while the
  first response is still active.
- Direct responses, followed redirects and a preserved 307 after the real 5 MiB
  replay cap exercise all three former early-release paths.
- Successful EOF and task cancellation return admission capacity and active-stream
  metrics to zero; normal terminal responses preserve status and body bytes.
- The existing overload, redirect, timeout, reset and flow-control regressions
  remain required, followed by pinned Clippy and hosted CI.

These tests exercise the real HTTP relay and the distributed semaphore's memory
implementation; they do not independently certify Redis lease renewal during a
network partition. Existing runtime-state lease tests remain the owner of that
backend behavior. Validation results and review status are recorded in the PR.

## Rollback

Revert this isolated change if necessary and restart/drain affected tunnel
processes under the usual deployment procedure. No schema, persisted state or
configuration-name migration is required. Reverting restores header-only
occupancy and its capacity-isolation limitation; it is not an equivalent fix.
