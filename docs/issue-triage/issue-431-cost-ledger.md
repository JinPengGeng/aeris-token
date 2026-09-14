# Issue #431: provider cost ledger plan

Date: 2026-09-14. This is a planning record, not an implementation decision.
It does not read production data, infer historical supplier charges, alter
settlement, or authorize a backfill.

## Objective and boundary

Build an auditable provider-cost ledger and margin report without conflating a
customer selling price with an actual supplier charge. Each reported cost must
retain its source, price version, currency conversion, confidence state and
access boundary. Existing usage, wallet and settlement behavior remains
unchanged until a separately reviewed implementation is accepted.

## Required inputs before implementation

| Dependency | Required evidence | Gate |
| --- | --- | --- |
| Supplier bill | Authorized, redacted bill/export fields that identify supplier, billing period, service/model or SKU, quantity, amount, currency and invoice/reference ID. Credentials, customer identities and raw invoice attachments stay outside the ledger. | Finance/procurement confirms the field-level redaction and retention policy. |
| Currency and FX | Billing currency plus a reproducible FX source, rate date/time, base/quote pair, rate value and retrieval/reference ID. | Finance approves the source and conversion convention; missing FX remains unconverted, not silently USD. |
| Price book | Versioned supplier price input with effective-from/to, supplier, SKU/model, unit, currency, source reference and approval owner. | A cost row can point to exactly one effective version or is not marked known. |
| Cost certainty | `known` means bill-backed and fully attributable; `estimated` means a versioned price-book calculation with explicit assumptions; `unknown` means evidence or attribution is missing. | UI, API and reports preserve the state and never aggregate estimated/unknown as confirmed actual cost. |
| Access | Finance/procurement may view source references and reconciliations; operators receive only the minimum redacted aggregates; ordinary users receive none. | RBAC review, audit logging and retention policy are approved before any source ingestion. |

## Minimal delivery sequence

1. Agree the redacted supplier-bill schema, price-book owner, FX source and
   retention/RBAC policy; capture sample data only in a non-production test
   fixture approved by finance/procurement.
2. Define an append-only ledger record that links usage attribution to its
   source facts and certainty state. Keep original currency and amount; derive
   a reporting currency only with the recorded FX fact.
3. Add a reconciliation view that separates known, estimated and unknown
   amounts, reports unmatched bill lines and prevents a margin total from
   presenting estimates as actuals.
4. Decide any settlement integration only after the ledger is independently
   accepted. No historical backfill or mutation is implied by this issue.

## Acceptance and release gates

Implementation is acceptable only when fixtures demonstrate: a bill-backed
known row, a versioned-price estimated row, an unknown row, an FX conversion
with its source/date, an expired or missing price version rejection, and
role-based denial of source detail. Reconciliation must identify unmatched
usage and bill lines without exposing credentials, customer identifiers or raw
supplier documents.

Before release, finance/procurement approves the field schema and redaction;
engineering approves attribution, version/effective-date selection and
idempotency; security approves RBAC, audit and retention; and the release
review confirms there is no settlement behavior change, production read, or
unreviewed historical backfill. A failed gate keeps the affected cost as
`unknown` or blocks ingestion; it must not be converted to `known` by default.

## Non-goals

- Changing customer prices, wallet balances, quotas or settlement decisions.
- Claiming historical provider cost or margin without retained source facts.
- Storing supplier credentials, raw invoices or customer identity data in a
  reporting ledger.
