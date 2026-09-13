# Issue #214: listener accept error recovery

## Decision

The fork's shared `HttpConnectionBudget::accept_with` helper already closes the
reported process-exit gap. Peer-level errors (`ConnectionRefused`,
`ConnectionAborted`, and `ConnectionReset`) retry immediately; resource or
unknown errors increment the counter, emit a structured error, and wait one
second before retrying. Both the configured gateway listener and the
compatibility `serve_tcp` entry point use this helper.

No runtime change is needed for this slice. The remaining parent Issue work is
tracked separately (target permit lifetime and Redis failure/error-contract
evidence); this record must not be read as full #214 closure evidence.

## Acceptance evidence

The frontdoor unit tests cover both branches:

- `http_connection_accept_peer_errors_retry_without_backoff` proves three peer
  errors do not incur the one-second resource backoff and increments the
  `accept_errors_total` metric.
- `http_connection_accept_retries_peer_errors_and_backs_off_resource_errors`
  proves peer errors retry and two resource errors consume exactly two seconds
  under Tokio's paused clock.

Run:

```sh
cargo test -p aether-gateway-frontdoor http_connection_accept --lib
```

Rollback is limited to reverting the additional regression test and this
decision record; no production behavior or data is changed.

Refs #214
