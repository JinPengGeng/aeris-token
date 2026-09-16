# Issue #225: README and ownership reference integrity

This bounded documentation slice removes two stale repository references and
adds a deterministic CI check for the same class of drift.

## Changes

- README navigation now resolves the `Q&A` entry to a real section with
  deployment and recovery answers.
- `.github/CODEOWNERS` now owns the actual `.github/CODEOWNERS` file and no
  longer names the absent root `release.toml`.
- `tests/readme_governance_reference_test.py` checks the five fixed README
  navigation entries and exact, non-wildcard CODEOWNERS paths using only the
  Python standard library. Rust CI runs it with the existing shell fixtures,
  including on default-branch pushes that change CODEOWNERS directly.

This does not claim that the full operations, multi-node, metrics scraping, or
production recovery contracts in #225 are complete. Those require their own
deployment evidence and remain open.

## Verification

```sh
python3 tests/readme_governance_reference_test.py
git diff --check
```
