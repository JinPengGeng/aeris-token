# Issue #431: provider cost ledger

Date: 2026-09-20.

This record describes the current local source tree; it is not evidence that
the code is merged, deployed, or backed by real supplier billing. The ledger
has administrator imports and an automatic Gateway capture path. The automatic
path creates a request-level provider-cost receipt only after eligible completed
usage reaches a final billing status. It does not backfill history or mutate
wallets, sales settlement, customer prices, or quotas. All examples and source
references in the companion API document remain synthetic.

## Delivered contract

- Versioned provider prices can be imported and listed. A price is keyed for
  overlap purposes by supplier, provider, model, dimension, currency, and unit;
  its effective interval is `[effective_from_unix_secs,
  effective_to_unix_secs)`. Reusing an `import_id` is idempotent only when the
  complete record is identical.
- Cost snapshots are explicitly imported. Their `import_id` has the same
  identical-retry behavior, and the database permits one snapshot per
  `(request_id, provider, model, dimension)`.
- The Gateway also creates one automatic `request` snapshot per eligible
  completed request. It uses `gateway-auto-provider-cost/<request_id>` as the
  import ID and `gateway_auto_capture` as `imported_by`; a replay retains the
  first matching request/provider/model/dimension receipt.
- Certainty is enforced at import time: `known` requires `supplier_bill`;
  `estimated` requires a matching, effective imported price; and `unknown`
  stores no provider amount, currency, or price reference.
- The summary endpoint groups sales and provider currencies, certainty, source
  kind, reconciliation status, and price version. It returns a margin amount
  only when the two currencies are identical.

## Operational boundary

The automatic path is conditional, not a historical scan. Settlement invokes
capture after a completed row has a final `settled` or `insufficient_quota`
billing status, including completed-row replay; it skips missing settlement
writers, non-terminal rows, failed/cancelled rows, and void usage. The
`attempt_funds` flow invokes the same capture only after its completed parent
lifecycle has been stored.

For capture, the local code requires a provider-cost data backend, a real
`provider_id`, a usable upstream model, and a representable sales amount. It
reads the `provider_cost_supplier_bindings` system config by provider ID, then
uses effective token prices and trusted usage metadata to estimate input,
output, cache-read, and cache-write components. A configured binding includes
the supplier, supplier currency, and `input_price_mode`. Missing bindings or
prices, untrusted/unsupported token usage, and image usage result in an
automatic `unknown` receipt when the prerequisite identity and sales amount are
available; image `per_image` estimation is not implemented. A missing backend,
provider ID, model, or representable sales amount produces no receipt.

The code does not import supplier bills, retrieve FX rates, or convert
currencies. Automatic snapshots are `estimated` from imported prices or
`unknown`; they are never supplier bills. Manual snapshot import remains the
route for a `known` `supplier_bill` record. Consequently, a same-currency
margin is an arithmetic ledger aggregate, not a production profit or financial
reconciliation assertion. This documentation does not claim that any real
supplier bill, automatic receipt, or financial evidence has been recorded.

There are four routes, all classified as the `billing_manage` control family
with the `admin:billing` authentication signature. Price and snapshot imports
are capped at 100 records per request and emit an administrator audit response.
See [Provider Cost API](../api/provider-costs.md) for request fields, status
rules, and synthetic examples.

## Remaining work outside this issue

Supplier-bill ingestion, FX fact storage and conversion, image per-price
estimation, historical backfill, reporting policy, and settlement use require
separate implementation and acceptance decisions. They are not implied by the
automatic capture path.

## Local verification record

On 2026-09-20, the local source was checked against
`apps/aether-gateway/src/state/runtime/provider_costs.rs`, its Gateway data
implementation, `crates/aether-usage/runtime/src/settlement_provider_cost_capture_tests.rs`,
and [Provider Cost API](../api/provider-costs.md). The settlement tests cover
capture after a confirmed completed settlement, replay of finalized usage,
skips for missing writers and ineligible usage, and propagation of capture
failure after settlement. This is a source and test-contract review only; it
does not assert a merged change, a deployed service, or real billing data.
