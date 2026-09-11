# #211 OpenAI Video Poll Decision

Date: 2026-09-12

## Scope

This record covers only the OpenAI video-task poll projection in
`crates/aether-video-tasks-core/src/openai.rs`. It does not resolve the
separate registry-retention work tracked by #211.

## Evidence And Decision

- The upstream provider reports active work as `in_progress`; the local
  projection previously recognized only `processing` and mapped every other
  value, including an absent status, to `Submitted`.
- A poll response can be sparse. The previous projection assigned
  `completed_at`, `expires_at`, error code, and result URL from every response,
  so an omitted field erased an earlier value.
- `LocalVideoTaskSnapshot::is_active_for_refresh` polls only Submitted, Queued,
  and Processing states. A terminal task must therefore never become active
  again because of an out-of-order poll response.

The projection now maps both `in_progress` and the legacy `processing` spelling
to local Processing. Missing or unknown status preserves the current local
state. A terminal local state rejects only a late active status, while allowing
an explicitly reported terminal outcome to evolve (for example, completed to
expired).

Optional timestamps and result URLs update only when the provider explicitly
includes their field. JSON `null` is an explicit clear for these optional
fields; omission is not. Failed, expired, and cancelled terminal states clear
the result URL. A failed response with an omitted `error` retains a known safe
error code or uses `provider_error`; an explicit `error: null` uses
`provider_error` so a stale prior error is not misattributed. Progress keeps its
existing value unless a provider value is present; only a completed response
without progress receives the existing 100% fallback.

## Verification

The unit tests in `openai.rs` cover:

- `in_progress` to Processing mapping;
- sparse polls preserving optional fields;
- explicit-null clearing and rejection of a late active poll after completion;
- terminal completed-to-expired evolution; and
- failed-terminal handling of explicit `error: null`.

Run:

```sh
rtk proxy env \
  CARGO_HOME=/tmp/aeris-rust.xKVBet/cargo \
  RUSTUP_HOME=/tmp/aeris-rust.xKVBet/rustup \
  PATH="/tmp/aeris-rust.xKVBet/cargo/bin:$PATH" \
  CARGO_TARGET_DIR=/tmp/aeris-video-tests \
  cargo test --locked -p aether-video-tasks-core openai::tests -- --nocapture
```
