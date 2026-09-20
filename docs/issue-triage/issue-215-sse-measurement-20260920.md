# SSE prefilter measurement — 2026-09-20

The current `StreamUsageFallback::observe_record` already skips ordinary
content-only records before deserializing a projected usage envelope. Nonblocking
logging and RPM cleanup were delivered earlier. This review compared the
existing substring filter with a proposed JSON-key scanner; the scanner was
not applied because it regressed representative short mixed workloads.

The standalone Rust benchmark uses `black_box`, the same records and real
`serde_json` parsing when each filter admits a record. It was compiled with
`rustc -O` using the workspace's cached dependency. The parse target is `Value`,
not Gateway's private projection, so these are filter/parser measurements,
not end-to-end request throughput claims.

| Workload | Existing ns/record | Proposed ns/record | Decision evidence |
| --- | ---: | ---: | --- |
| Short content | 98 | 98 | No measured gain |
| 64 KiB content | 48,105 | 28,258 | Scanner helps this large-record case |
| Content containing usage/tier keywords | 5,407 | 11,267 | Scanner is slower despite avoiding all 5,000 parses |
| 98% content, 1% usage, 1% finish | 35 | 41 | About 17% slower |
| 98% content, 1% usage, 1% error | 34 | 42 | About 24% slower |
| Usage, escaped keys, image and error | 265 | 266 | No measured gain |

Keep the current production prefilter and semantic tests. A custom key scanner
or serialization cache is not justified by this result. The measurement does
not block other issue development or require further fault drills.

Scratch evidence and reproducible source:
`/Users/jinpeng/.agents/tmp/aeris-resume-20260920/shared-scheduler/issue-215-sse-prefilter-bench.rs`
and `run-issue-215-sse-prefilter-bench.sh` in the same directory.
