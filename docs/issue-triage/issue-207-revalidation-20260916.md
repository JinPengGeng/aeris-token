# Issue #207 revalidation (2026-09-16)

This is an evidence-only revalidation of the three high-risk runtime claims in
Issue #207 at the current integrated head `49fddbdc7` (with this documentation
slice recorded in the following commit). No tunnel source or active PR branch
is changed by this slice.

## Result

All three claims are already covered by the current implementation. This slice
does not duplicate those fixes.

| Claim | Current boundary | Regression evidence |
| --- | --- | --- |
| Internal error details exposed to clients | `apps/aether-gateway/src/error.rs` maps `GatewayError::Internal` to a fixed `internal server error` response. The server log keeps only a fingerprint and length by default; opt-in detail logging applies URL/credential redaction. | `error::tests::internal_errors_do_not_expose_internal_details`, `error::tests::internal_error_detail_logging_redacts_without_truncating`, and `error::tests::internal_error_logging_omits_details_when_disabled` |
| No graceful gateway shutdown | `apps/aether-gateway/src/main.rs` uses a `CancellationToken`, stops accepting connections, drains active HTTP/upgrade connections, force-closes after the configured deadline, and then shuts down usage/background runtimes. | `shutdown::gateway_shutdown_drains_in_flight_http1_and_http2_responses`, `shutdown::gateway_shutdown_closes_idle_protocol_detection_connections`, `shutdown::gateway_shutdown_force_cancels_a_handler_without_socket_io`, and `shutdown::gateway_shutdown_force_closes_upgraded_io_and_waits_for_release` |
| Windsurf unbounded stream channel/aggregation | `apps/aether-gateway/src/execution_runtime/windsurf.rs` uses `mpsc::channel(WINDSURF_STREAM_FRAME_CHANNEL_CAPACITY)`, applies `append_windsurf_delta_with_limit` to decoded content, and reports a full channel as a cancellation/error boundary. | `execution_runtime::windsurf::tests::windsurf_delta_buffer_rejects_overflow_without_mutation` plus the bounded-channel send path at `send_stream_frame` |

The remaining #207 items are architecture/lint/lease follow-ups already listed
as P2 in `docs/issue-triage/security-runtime-audit.md`; they are outside this
runtime revalidation and should not be mixed into the completed security fixes.
