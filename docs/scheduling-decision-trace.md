# Scheduling decision trace

The gateway emits `scheduling_decision_trace` events for the actual local scheduling path.
Every event carries `schema_version = 1` and the request `trace_id`, so operators can order
the page, selection, gate, classifier, budget, and termination decisions for one request.

## Decisions

- `candidate_page`: scheduler affinity generation, page ordinal, selection source, bounded
  eligible/skipped candidate decisions, skip reason, and rank vector.
- `attempt_selected`: request-wide observed attempt ordinal, candidate/retry/pool indexes,
  pool sticky/score/cursor source, rank vector, and consumed observed-attempt count.
- `dynamic_gate`: send-path gateway execution gate result and denial reason.
- `classifier_disposition`: status, classifier result, retry action, failure scope, token action,
  and upstream-error preservation policy.
- `budget_decision`: provider transfer count/deadline exhaustion on current `main`. The shared
  `AttemptBudget` child issue can map its additional counters into this decision without
  changing the envelope.
- `termination`: distinct `responded`, `deferred`, `no_path`, `candidates_exhausted`,
  `planning_error`, and `error` reasons with the final observed attempt count.

Candidate arrays are capped at 64 entries per event and include an omitted count. Labels are
capped at 96 UTF-8 bytes. Candidate identity fields are deterministic SHA-256-derived opaque
references, truncated to 96 bits for observability correlation. They are not credentials and
must not be used for authorization.

## Privacy contract

The trace schema accepts only typed scheduler fields. It never accepts or serializes the full
report context, request body, headers, upstream response body, decrypted credential, bearer or
session token, sticky token, pool lease token/owner/key, proxy configuration, or URL/query.
Provider, endpoint, model, and credential database identifiers are emitted only as namespaced
opaque references. `token_action` is an enum describing classifier behavior, never token data.

Do not add generic JSON maps to this schema. New fields must be typed, bounded, and covered by
a negative secret-leak test before they can be emitted.

## Current-main compatibility

This envelope is ready for the scheduler P0 contracts that are not yet present on `main`.
Until those child changes merge, `generation` uses the scheduler affinity epoch, dynamic gate
events cover the gateway upstream-execution gate, and budget events cover observed attempts plus
provider-transfer exhaustion. Canonical snapshot generation, probe-lease results, the complete
dynamic gate set, and shared `AttemptBudget` counters can populate the same typed decisions later
without changing the privacy boundary.
