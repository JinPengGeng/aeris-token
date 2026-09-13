# Issue #225 tunnel documentation audit

Audited against `origin/main` at `c860eec66` on 2026-09-13.

The tunnel installer and README intentionally default to `fawney19/Aether`:
that repository publishes the `tunnel-v*` artifacts (the fork currently
publishes gateway releases but no tunnel tag). The README's `tunnel-v0.3.17`
links therefore remain valid. The environment reference is generated from the
clap command metadata and remains the authoritative list for runtime options;
the three installer-only variables are documented separately.

The only drift found in the existing Issue #225 decision record was its stale
fork commit identifier. It now records the current main commit. No runtime
configuration, release source, or network behavior was changed.
