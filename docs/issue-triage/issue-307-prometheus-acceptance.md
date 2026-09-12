# Prometheus parser and alert acceptance

Issue: #307; parent #217. Fork-only implementation, based on `f663b48a9`.

## Revalidated report and decision

Official Prometheus 3.14.0 tooling accepted the existing four alert rules but
rejected the real `logging_metric_samples()` text with `second HELP line for
metric name "aether_gateway_billing_fail_open_total"`. The renderer declared
HELP/TYPE separately for the daily-quota and RPM samples of one family. RED
families with multiple label sets share the same renderer and failure mode.

Emit metadata once per family, without changing sample values, labels or
ordering. Add an executable billing export fixture and Prometheus parser CI.
This is a medium-sized shared monitoring fix; it changes no billing decision.

The rule audit also found that enrichment/fail-open aggregation removed the
component/operation labels referenced by the runbook, settlement counters had
no alert, and provider documentation incorrectly promised configured provider
IDs. Preserve routing labels, cover both usage and video settlement, and
describe the normalized provider-type terminal outcome accurately.

## Verification and review boundary

Prometheus rule fixtures cover positive and negative conditions, exact pending
windows, counter reset, recovery, and output routing labels. The billing
fixture uses actual production recorders and exporter; CI pins the Prometheus
archive checksum and uploads its output and validation logs.

Keep #307 open until failure injection proves each production producer path
and the complete deployment scrape/alert acceptance has evidence. This change
does not prove unique failed-event counts: the counters count entry into a
failure branch, including retries that fail again. No production environment
is accessed by these tests.

Rollback is a revert of this focused commit. That restores the known duplicate
metadata parser failure; preserve the last validated exporter if downstream
monitoring depends on these families. Alert rule rollback is independent of
runtime accounting, and never requires deleting usage or DLQ records.
