# Issue 343: billing and quota error contract

Decision date: 2026-09-13. Repository: `JinPengGeng/aeris-token` only.
Parent work: #247 and #254. This supersedes the wallet portion of the
[247/254 decision](issue-247-254-api-contract-decision.md).

## Accuracy, benefit and scope

The report is accurate. Before this change, the wallet denial builder used
the transient `RateLimit` kind. OpenAI emitted
`429/rate_limit_error/insufficient_quota`; Claude emitted
`429/rate_limit_error` with a message containing the remaining balance.
Stream and finalize classifiers did not recognize exhausted credit, so simply
changing the response builder would lose the distinction during conversion.

Priority P1, benefit high, risk medium, size M: the change prevents account
errors from masquerading as transient limits and removes balance disclosure
from the supported public envelopes. Authorization, wallet arithmetic,
settlement, notification and refund decisions are unchanged. Gemini remains
outside this decision; its RESOURCE_EXHAUSTED classification is retained.

## Agreed public contract

The implementation follows the repository owner's recorded
[decision](https://github.com/JinPengGeng/aeris-token/issues/343#issuecomment-5647563970).
References used for that decision: [OpenAI error codes](https://platform.openai.com/docs/guides/error-codes/api-errors)
and [Anthropic errors](https://platform.claude.com/docs/en/api/errors).

| Reason | OpenAI HTTP/type/code | Claude HTTP/type/code | Retry advice |
| --- | --- | --- | --- |
| Credit exhausted | 429 / insufficient_quota / credit_balance_exhausted | 402 / billing_error / balance_exceeded | Restore credit; omit Retry-After |
| Tenant/key permission denied | 403 / permission_error | 403 / permission_error | Change access policy; omit Retry-After |
| Provider rate limit | 429 / rate_limit_error | 429 / rate_limit_error | Bounded backoff; preserve a known wait, invent none |

Both quota envelopes use the public message `Insufficient quota` and retain
the response `x-trace-id`. They never include remaining balance, credentials,
user/key identifiers or raw billing detail. `balance_exceeded` is an optional
gateway extension to Claude's standard envelope, intentionally fixed here for
correlation with the older internal category.

## Implementation and review boundaries

- Add `QuotaExhausted` to the existing shared error kind. The shared formatter
  owns protocol-specific quota types/codes, safe message and HTTP status.
- Recognize exact structured quota type/code hints before generic 429 handling.
  Legacy `insufficient_quota` and `balance_exceeded` codes remain accepted as
  input. Arbitrary message text never establishes exhausted credit.
- Use that classification for local wallet responses, protocol stream
  conversion, Claude precommit stream status, and finalization. Converted
  billing errors select the client's HTTP status, not the provider's status.
- Strip Retry-After on quota and permission finalizations, case-insensitively.
  Provider rate-limit finalizations retain a supplied wait and leave it absent
  when unknown. Timed daily/plan/RPM limits keep their existing behavior.
- A rejection before streaming starts is a JSON HTTP error. After the stream
  commits, only the terminal event's envelope can change; HTTP status is fixed.

## Acceptance evidence

The reviewable `docs/api/fixtures/public-api-compatibility.json` matrix covers
Chat, Responses, Embeddings and Claude for exhausted credit, denied permission,
and known/unknown provider rate-limit waits. Gateway tests use the actual
router for wallet/key denial, include streaming requests, and assert trace,
status, envelope and absence of private balance information. Finalize tests
exercise both provider formats, successful HTTP statuses carrying errors, and
preconverted client bodies. Stream tests exercise Chat/Responses/Claude
conversion and Claude precommit billing classification.

Focused Rust tests, formatting, Clippy and the fork's four required CI gates
(Rust, Frontend, Automation Policy, Dependency Audit) must pass before merge.
Exact command results and review outcome are recorded in the PR.

## Client migration and rollback

Consumers matching `error.code=insufficient_quota` must also accept the new
OpenAI `credit_balance_exhausted` code and preferably branch on
`error.type=insufficient_quota`. During rolling upgrades accept both codes.
Claude clients must accept `402/billing_error` instead of assuming wallet
denial is a 429 rate limit. Treat both as requiring an account change.

Some OpenAI SDKs retry every HTTP 429, regardless of Retry-After. This gateway
contract does not suppress SDK retries by itself: clients should inspect the
quota type/code and configure their retry wrapper or SDK retry count to stop
unchanged account-error retries. Transient provider limits remain retryable
with bounded backoff, subject to the operation's existing idempotency rules.

This change requires no data migration or configuration switch. Rollback is a
revert of the PR's code, fixtures and current API documentation followed by
redeployment. It restores the older OpenAI code and Claude 429 behavior,
including the old Claude message's balance disclosure; prefer a forward fix
if reverting for a client-specific incompatibility. Keep dual-code consumers
until every deployment version is confirmed upgraded or rolled back.
