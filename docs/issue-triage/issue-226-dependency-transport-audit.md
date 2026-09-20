# Issue #226: dependency and transport audit

Revalidated against the current checkout on 2026-09-20. This records the
decision boundary for the remaining dependency work; it does not claim a
transport migration.

## Closed formula and metric items

- `crates/aether-billing/src/formula_engine.rs` rejects non-finite arithmetic
  intermediates and quantized totals or `_cost` breakdown values. Its unit
  tests cover zero divisors, masked non-finite values, arithmetic overflow,
  quantization overflow, and the finite boundary. The decision and scope are
  recorded in `formula-finite-decision.md`.
- `FORMULA_ALLOWED_FUNCTIONS` is the single function declaration shared by the
  evaluator and admin validation. `formula-allowlist-decision.md` records the
  per-function and rejection coverage.
- Gateway stage latency already exports cumulative finite buckets. This slice
  adds the `le_ms="+Inf"` bucket so observations above 10 seconds remain in the
  exported distribution. The unit test exercises an over-range observation and
  verifies that the terminal bucket equals the total count.

The stage-latency family uses the repository's existing `le_ms` label and
counter renderer. It is a bounded distribution for the gateway dashboard, not
a newly declared Prometheus native histogram API; changing its public family
shape requires a separate compatibility decision.

## Current dependency proof

The workspace root pins `wreq = "=6.0.0-rc.28"` and
`wreq-util = "=3.0.0-rc.10"`. Gateway WebSocket sessions converge on
`handlers/proxy/websocket/transport.rs::connect_upstream_websocket`, which
constructs the browser-capable wreq client and returns `wreq::ws::WebSocket`.
Live and Responses session modules call that helper. Replacing wreq therefore
changes upstream handshake, proxy, TLS fingerprint, frame and relay behavior;
it is not a dependency-only update.

`cargo tree -p aether-gateway -i aws-lc-rs -e features` shows that aws-lc is
still intentional in the effective Gateway closure:

- `aether-crypto` directly depends on `aws-lc-rs` for compatibility helpers.
- `object_store` with the enabled `aws` feature brings reqwest 0.13 with its
  rustls/aws-lc path.
- The direct workspace `rustls` declaration enables `ring`, but omits
  `default-features = false`; Cargo therefore also requests rustls defaults.

Making only that final declaration explicit would not remove aws-lc from the
current binary, so it is not a meaningful completion claim. `Cargo.lock` was
last updated by the rustls security fix to rustls 0.23.45 and aws-lc-rs 1.18.1;
this audit does not change versions or lock resolution.

## Decision and acceptance

Do not replace the WebSocket transport or remove aws-lc in a broad dependency
cleanup. There is no current ADR that defines an equivalent replacement for
wreq's browser emulation and WebSocket behavior, and the existing
`issue-235-adr-decision.md` explicitly leaves that decision open.

Before selecting a pin, upgrade, isolation, or replacement, collect all of the
following from a clean locked build:

1. `cargo tree -p aether-gateway -i aws-lc-rs -e features` and
   `cargo tree -p aether-gateway -i wreq -e features` artifacts.
2. Release build timing and binary section-size comparison for the proposed
   closure change.
3. The existing direct, HTTP-proxy, SOCKS5, SOCKS5h, WSS/SNI, browser-profile,
   frame-size, cancellation, and upstream-idle WebSocket checks against the
   candidate transport.

Only then can a migration proposal define the supported feature matrix,
rollout, and rollback. Until that evidence exists, retain the exact RC pin and
keep #226 open.
