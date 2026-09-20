# Provider Cost API

This control-plane API imports provider prices and supplier-cost receipts and
reads their summaries. After completed settlement, the Gateway also estimates
token costs from configured supplier bindings and effective prices, as described
below. Supplier bills require explicit import; currency conversion and wallet
changes are outside this API. The example identifiers, amounts, references,
and curl commands below are synthetic; do not treat them as production billing
evidence or run them against a service without supplying the appropriate local
endpoint and credential.

## Access and routes

All routes are in the `billing_manage` control family and have authentication
signature `admin:billing`. A management token therefore needs the billing
permission appropriate to its HTTP method: `admin:billing:read` for `GET` and
`admin:billing:write` for `POST`.

| Method | Path | Request | Response |
| --- | --- | --- | --- |
| `GET` | `/api/admin/billing/provider-costs/prices` | Optional `page` (1--100000), `page_size` (1--200; default 50) | `{ "items", "page", "page_size" }` |
| `POST` | `/api/admin/billing/provider-costs/prices/import` | `{ "prices": [...] }`, 1--100 items | Success: `{ "items", "inserted", "already_exists" }` |
| `GET` | `/api/admin/billing/provider-costs/snapshots` | Required Unix-second `from`, `until`; `until > from` | `{ "items", "occurred_from_unix_secs", "occurred_until_unix_secs" }` |
| `POST` | `/api/admin/billing/provider-costs/snapshots/import` | `{ "snapshots": [...] }`, 1--100 items | Success: `{ "items", "inserted", "already_exists" }` |

An import response returns each stored record with `inserted: true` when newly
written, or `inserted: false` when the same `import_id` and identical content
was already present. Reusing an `import_id` with different content is rejected.
For snapshot imports, the returned record is the receipt for that import. A
snapshot may progress from `unknown` to `estimated` or `known`, and from
`estimated` to `known`. The request/provider/model/dimension identity retains
one current row, which contributes to summaries once. Sales amount, sales
currency and occurrence time remain fixed. A different import cannot replace
an already known cost. Immutable import history is retained in
`provider_cost_snapshot_imports`; replaying an older import returns its original
receipt without downgrading the current row.

Apply `20260919000000_add_provider_cost_snapshot_imports.sql` before deploying
the writer that supports upgrades. It copies existing snapshot imports into
the receipt table without changing current costs. For an application rollback,
retain that additive table and pause snapshot imports: an older writer cannot
recognize historical import IDs after a snapshot has advanced. Schema removal
would discard that replay history and is outside the application rollback.

`imported_by` may be omitted. The handler supplies an empty value before
deserialization, then overwrites it with the authenticated administrator's user
ID, or `management_token` when there is no administrator principal. A client
value is likewise overwritten.

Imports are processed in request order and are not a single batch transaction.
If an individual repository commit fails after earlier items completed, the
response has the failure status and this body:

```json
{
  "detail": "provider cost import was rejected",
  "partial": true,
  "completed": [{ "record": {}, "inserted": true }],
  "failed_index": 1,
  "retry_safe": true
}
```

`completed` contains the successful outcomes before `failed_index`; it is an
empty array and `partial` is `false` when the first item fails. The server marks
the response `retry_safe: true`, so a client can retry with the same import IDs.
Completed items are idempotent and return `inserted: false` on an identical
retry. A partial response with completed items also carries the administrator
audit response for the completed portion.

An import-ID content conflict returns `409` with a redacted conflict detail.
Contract or validation rejection returns `400`, and a repository/storage failure
returns `503`; neither response exposes the underlying storage error.

## Amount representation

`price_units`, `sales_amount_units`, and `provider_cost_amount_units` are Rust
`u64` values persisted as PostgreSQL `BIGINT`. They use the billing fixed-point
scale `1e8`: `100000000` represents `1` unit of the associated currency.
`margin_amount_units` is signed (`i64`) because sales minus cost may be
negative.

JSON numbers above JavaScript's safe integer limit (`9007199254740991`) lose
precision in ordinary `Number` clients. Preserve these values as JSON integer
lexemes and parse them with a bigint-aware client, or use a serializer that does
not round integers. The current Rust contract declares numeric `u64`/`i64`
fields; it does not declare that quoted numeric strings are accepted.

## Importing a price

A price contains `supplier`, `provider`, `model`, `dimension`, `currency`,
`unit`, `version`, its fixed-point `price_units`, an effective window, and a
source reference. Dimensions are `input`, `output`, `cache_read`,
`cache_write`, `image`, and `request`; units are `per_million_tokens`,
`per_image`, and `per_request`.

The interval is half-open: a price applies when `effective_from_unix_secs <= at`
and, when set, `at < effective_to_unix_secs`. Price windows cannot overlap for
the same supplier/provider/model/dimension/currency/unit identity.

```bash
# Synthetic example only. Set AERIS_BASE_URL and AERIS_ADMIN_TOKEN locally.
curl -X POST "$AERIS_BASE_URL/api/admin/billing/provider-costs/prices/import" \
  -H "Authorization: Bearer $AERIS_ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  --data @- <<'JSON'
{
  "prices": [
    {
      "import_id": "synthetic-price-input-v1",
      "supplier": "synthetic-supplier",
      "provider": "synthetic-provider",
      "model": "synthetic-model",
      "dimension": "input",
      "currency": "USD",
      "unit": "per_million_tokens",
      "version": "synthetic-v1",
      "price_units": 125000000,
      "effective_from_unix_secs": 1767225600,
      "effective_to_unix_secs": 1769904000,
      "source_reference": "synthetic-price-book-v1"
    }
  ]
}
JSON
```

## Importing snapshots

Every snapshot has an import ID, request ID, provider, model, dimension, sales
amount and currency, certainty, source kind, reconciliation status, and
occurrence time. Valid reconciliation statuses are `unreconciled`,
`matched`, `disputed`, and `not_applicable`.

| Certainty | Required state |
| --- | --- |
| `known` | `provider_cost_amount_units`, nonempty `provider_currency`, and nonempty `source_reference`; `source_kind` must be `supplier_bill`. Price references are optional. |
| `estimated` | Amount, currency and nonempty `source_reference`; `source_kind` must be `estimate`. Supply either the legacy `price_import_id` and `price_version`, or nonempty `price_components` on a `request` snapshot. Referenced prices must match provider, model, dimension, currency, version and effective occurrence interval. |
| `unknown` | `provider_cost_amount_units`, `provider_currency`, `price_import_id`, and `price_version` must all be JSON `null`; `price_components` must be empty. `source_reference` may be `null`. |

Legacy single-price imports validate the price identity and time window but do
not derive the submitted amount. Request aggregates additionally validate each
component's quantity, unit and fixed-point amount against its price, rounding
up to the smallest stored currency unit. Component amounts must sum to the
estimated request cost, and every component must reference the same supplier.

`price_components` defaults to `[]` for older clients. Each component contains
`dimension`, `quantity`, `unit`, `price_import_id`, `price_version`,
`price_source_reference` and `amount_units`; dimensions must be unique. Input,
output and cache components share one `dimension: request` snapshot so sales
are counted once. The current row and immutable import receipt both preserve
the components. A later supplier invoice may retain that estimate provenance
while recording a different known cost; replaying the estimate cannot replace
the invoice. Migration `20260920000000` adds the component columns and updates
the certainty constraints for existing installations.

The billing library provides checked fixed-point estimation and explicit
cache-token normalization. After a completed Gateway usage row has a confirmed
terminal settlement, the Gateway writes one automatic `request` snapshot when
the row has a real `provider_id` and a representable sales amount. The system
config value `provider_cost_supplier_bindings` is a provider-ID map; each entry
must declare the supplier, supplier currency and input-price semantics, for
example:

```json
{
  "provider-id-123": {
    "supplier": "synthetic-supplier",
    "currency": "USD",
    "input_price_mode": "exclusive_of_cache"
  }
}
```

The automatic record has `source_kind: estimate` and
`imported_by: gateway_auto_capture`; it never claims to be a manual import or a
supplier bill. It uses the persisted provider ID, mapped upstream model and a
trusted `usage_available: true` marker. Missing supplier configuration, a
missing or incompatible price, untrusted token usage, or unsupported cache
semantics produces an `unknown` automatic snapshot rather than estimating from
zeros. A row without a provider ID is skipped because a display `provider_name`
is never used as pricing identity. The first persisted request snapshot is
retained on replay, so price changes do not replace an earlier estimate or
downgrade a later known receipt.

The automatic path currently estimates input, output, cache-read and
cache-write token prices. It does not estimate image `per_image` prices. Funded
image `attempt_funds` trigger the same automatic receipt check only after the
completed parent lifecycle is persisted, never on an intermediate attempt;
image usage remains `unknown` until
per-image supplier price capture is supported.

```json
{
  "snapshots": [
    {
      "import_id": "synthetic-snapshot-estimated-v1",
      "request_id": "synthetic-request-001",
      "provider": "synthetic-provider",
      "model": "synthetic-model",
      "dimension": "input",
      "sales_amount_units": 250000000,
      "sales_currency": "USD",
      "provider_cost_amount_units": 125000000,
      "provider_currency": "USD",
      "certainty": "estimated",
      "source_kind": "estimate",
      "reconciliation_status": "unreconciled",
      "price_import_id": "synthetic-price-input-v1",
      "price_version": "synthetic-v1",
      "source_reference": "synthetic-estimate-001",
      "occurred_at_unix_secs": 1768000000
    },
    {
      "import_id": "synthetic-snapshot-known-v1",
      "request_id": "synthetic-request-003",
      "provider": "synthetic-provider",
      "model": "synthetic-model",
      "dimension": "output",
      "sales_amount_units": 300000000,
      "sales_currency": "USD",
      "provider_cost_amount_units": 150000000,
      "provider_currency": "USD",
      "certainty": "known",
      "source_kind": "supplier_bill",
      "reconciliation_status": "matched",
      "price_import_id": null,
      "price_version": null,
      "source_reference": "synthetic-known-source-003",
      "occurred_at_unix_secs": 1768000200
    },
    {
      "import_id": "synthetic-snapshot-unknown-v1",
      "request_id": "synthetic-request-002",
      "provider": "synthetic-provider",
      "model": "synthetic-model",
      "dimension": "request",
      "sales_amount_units": 100000000,
      "sales_currency": "USD",
      "provider_cost_amount_units": null,
      "provider_currency": null,
      "certainty": "unknown",
      "source_kind": "manual_import",
      "reconciliation_status": "not_applicable",
      "price_import_id": null,
      "price_version": null,
      "source_reference": null,
      "occurred_at_unix_secs": 1768000100
    }
  ]
}
```

To post the synthetic JSON shape above, use the snapshot import route with the
same headers as the price example and `--data @snapshot.json`. Do not label a
record `known` unless it carries a real `supplier_bill` source kind and the
required source reference; no real supplier bill is represented in this
document.

## Summary semantics

`GET /api/admin/billing/provider-costs/snapshots?from=...&until=...` uses the
half-open occurrence window `[from, until)`. Each item is grouped by
`sales_currency`, `provider_currency`, `certainty`, `source_kind`,
`reconciliation_status`, and `price_version`, and contains:

- `sales_amount_units` and optional `provider_cost_amount_units`
- optional `margin_amount_units`
- `snapshot_count`, `unreconciled_count`, and `unknown_count`

`margin_amount_units` is implemented only when `sales_currency` equals a
non-null `provider_currency`; it is `SUM(sales_amount_units) -
SUM(provider_cost_amount_units)`. It is `null` for different currencies or an
unknown provider cost. There is no FX conversion or cross-currency margin.
Because the query groups by certainty and source kind, it may return arithmetic
margins for estimated rows; consumers must preserve those labels and must not
present the result as audited profit.
