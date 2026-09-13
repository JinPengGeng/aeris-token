# Issue #225 tunnel documentation audit

Initial audit baseline: fork commit `c860eec66` on 2026-09-13.
The conclusions below were rechecked against fork commit
`bf03bbd65072e416975d16c129abe7e83105b07d` on the same date. These are
recorded audit snapshots; they do not claim to track the moving main branch.

The tunnel installer and README intentionally default to `fawney19/Aether`:
that repository publishes the `tunnel-v*` artifacts (the fork currently
publishes gateway releases but no tunnel tag). The README's `tunnel-v0.3.17`
links therefore remain valid. The environment reference is generated from the
clap command metadata and remains the authoritative list for runtime options;
the three installer-only variables are documented separately.

The existing Issue #225 decision record now identifies its historical audit
baseline explicitly. No runtime
configuration, release source, or network behavior was changed.
