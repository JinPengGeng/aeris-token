# Issue #48 conversation-history contract

The current resolver and history paths implement the capability contract.
PR #40 was closed without merging; this record establishes coverage from
current source and tests. Deployment and parent-issue closure remain separate.

- `ConversationHistoryResolver` declares `Native` for Responses-to-Responses,
  `Hydrate` for Responses-to-Chat, `Translate` for Responses-to-Claude/Gemini,
  and rejects every other continuation pair. Candidate planners record an
  explicit `conversation_history_unsupported` skip reason so another capable
  candidate may be selected.
- Materialized gateway history requires both the authenticated tenant and API
  key. Records, lookup keys, cache entries, and persisted payload fingerprints
  share that scope; partial identities produce no record and cannot perform a
  local lookup.
- A native `previous_response_id` is a provider-owned handle. It is forwarded
  when gateway-local history and even a local history scope are absent. When a
  scoped native record is available, its provider, endpoint, and credential
  binding can only exclude a mismatched candidate; it never becomes a required
  local transcript.

Focused selectors:

```text
cargo test -p aether-ai-formats resolver_declares_each_history_capability_and_rejects_lossy_pairs
cargo test -p aether-ai-formats tenant_and_api_key_both_partition_history
cargo test -p aether-ai-formats all_history_capabilities_reject_unscoped_records_and_lookups
cargo test -p aether-ai-formats native_continuation_preserves_the_provider_owned_response_id
cargo test -p aether-gateway native_continuation_allows_missing_local_history
cargo test -p aether-gateway native_continuation_allows_missing_local_scope
cargo test -p aether-gateway native_continuation_rejects_persisted_binding_mismatch
```

The gateway candidate paths are
`planner/standard/openai/responses/decision/request.rs` and
`planner/standard/family/request.rs`; both consume the resolver result before
conversion or upstream dispatch.
