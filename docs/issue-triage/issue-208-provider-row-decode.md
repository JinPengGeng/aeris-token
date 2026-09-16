# Issue #208 provider-key row decode boundary

The PostgreSQL provider-key list queries select the complete `provider_api_keys`
mapping used by `map_key_row`. Optional fields remain optional when SQL returns
`NULL`, but a missing column or an incompatible SQL type is a data-layer error.
The mapper therefore reads every selected field through `row_get` and returns
that error to `collect_query_rows`; it does not turn a decode failure into a
missing value or a default.

The regression contract is covered by
`provider_catalog_key_row_mapping_propagates_decode_errors` in
`crates/aether-data/adapters/postgres/src/provider_catalog.rs`. It scopes the
check to `map_key_row` and rejects direct `try_get(...).ok()` or equivalent
error swallowing in that recovery/list path.

Validation:

```text
cargo test -p aether-data-postgres provider_catalog_key_row_mapping_propagates_decode_errors --lib
```

This is a bounded adapter slice. It does not claim that historical production
rows are clean or close parent Issue #208; those still require the separate
read-only historical audit and production evidence.
