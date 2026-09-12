# Issue #300: bounded image quote and frozen calculation

Status: implementation for review. The accepted financial design is recorded in
[Issue #300](https://github.com/JinPengGeng/aeris-token/issues/300#issuecomment-5648866414).
The funds adapter is developed separately; this change does not reserve money or
restore paid-image admission through the gateway.

## Problem and chosen contract

The existing authorization input has no image dimensions, and paid images return
an unknown estimate. Separately, an unmatched matrix entry previously produced a
complete calculation with zero image cost. A finite image authorization needs to
cover output count, image and per-image request charges, provider and API-key
multipliers, processing catalogs and every billable token/cache component.

`quote_image_authorization` accepts typed image dimensions and returns a frozen
`BillingImageAuthorizationQuote`. It reuses the existing matrix → pixel range →
explicit default resolver and the actual settlement calculator. When an image
catalog exists but does not match an output, normal settlement now records
`NoRule` with `image_output_pricing`; a missing price is never declared a complete
zero-cost image calculation. Intentional token-only image pricing remains valid.

The caller supplies the exhaustive set of outputs permitted by the **final**
provider/model projection. Explicit size/quality must match every supplied output;
auto/default dimensions are bounded by the most expensive supplied outcome.
Missing sets, unpriced outcomes and unproven token costs return no quote. The
caller must not substitute catalog entries for proof of the provider's output
capabilities. The current contract limits count to 1–10 and honors lower projection
limits; it limits the exhaustive output set to 64 concrete variants.

Token bounds cover the whole operation, including image inputs, edits, previews,
cache and all generated images. Text length alone cannot establish them. The
calculator explores reachable non-monotonic context tiers and cache classifications,
including every explicit cache TTL price in the selected catalogs. Explicit free
providers and zero API-key multipliers need no token-cost bound, but malformed
pricing configuration and invalid dimensions still fail validation.

The quote stores a version, the pricing snapshot and normalized inputs. Amounts use
100,000,000 units per USD. Authorization rounds upward with checked integer bounds;
normal settlement retains the existing eight-decimal rounding. The serialized
quote belongs to a server-owned funds reservation and is not client authorization.

## Actual output and financial integration

`calculate_image_with_quote` uses the frozen snapshot even if current model prices
change. It accepts actual output and usage evidence, computes the full actual cost,
and exposes collectible units capped at the quote plus any excess requiring
reconciliation. Fewer completed outputs incur only their actual charges. Priced
overruns retain cost evidence and never silently increase the collectible amount.
Missing or unpriced evidence returns no calculation: keep the funds hold pending
reconciliation instead of inventing zero output or the requested count.

The formats crate adds `NormalizedOpenAiImageRequest::authorization_dimensions()`.
It reuses existing JSON/multipart normalization and exports count/limit, normalized
size/quality, operation, output format, partial count and image/mask presence. It
contains no prompt, image bytes, URLs or user field. These are pre-projection
dimensions; transformations such as ChatGPT-Web size conversion still need to
provide their final effective dimensions before quoting.

The legacy `estimate_authorization_cost_upper_bound` continues to return unknown
for paid images. Gateway admission, sync/stream/tunnel projection, frozen quote
transport, actual partial-output capture, cancellation semantics and transactional
funds settlement remain integration requirements. In particular, the existing
event-enrichment cancellation behavior is not changed by this API.

## Review and acceptance

Local verification covers count 1/10 and lower model limits; matrix/range/default
precedence; complete auto sets and missing coverage; both multipliers; token/cache
bounds and unusual TTLs; non-monotonic tiers and processing multipliers; frozen
pricing, partial output, overrun/capped collection; free/zero-cost cases; invalid
amounts, dimensions and integer overflow; JSON and multipart normalized dimensions.
Full billing tests and relevant image-format tests must pass with Rust 1.95.0.

Local evidence on 2026-09-13: all 109 billing library tests and 110 image-related
format tests passed. Clippy for both crates and all targets passed with warnings
denied; rustfmt and diff checks passed. Independent review and hosted required
checks are still pending.

Keep this PR in draft until independent review and its required checks pass. Do
not close #300 on this component: real PostgreSQL reservation races, every wallet
debit path, insufficient-quota recovery, request lifecycle integration and crash
recovery are separate mandatory acceptance evidence. There is no database
migration in this quote component. Roll back its consumers before reverting the
quote schema; preserve any frozen quote referenced by an active reservation.
