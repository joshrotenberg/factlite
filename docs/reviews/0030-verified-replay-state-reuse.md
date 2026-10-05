# 7.3 Verified replay state reuse — self-review

Reviewed the complete production, test, benchmark and documentation diff.
No independent review or remote CI ran.

## Finding and change

Profiling demonstrated a redundant transition for every keyed record: canonical
request evaluation already produces the next state, but replay discarded it and
applied/validated the matching report again. Report application consumed roughly
half the profiled wall time in larger fixtures. Reuse the evaluated model only
after complete generated/persisted report equality; unkeyed records continue
through report application. Format, identity, row/event and canonical checks run
in the same order, within the original short SQL snapshot.

Self-review checked the inductive validity argument: the empty model is valid;
evaluation checks declared schema/allocation, types and known references,
pre-state conditions, declarative cardinality and final uniqueness. It preserves
untouched valid facts and generates canonical schema, IDs and effective events.
Exact full-report equality then proves the same retained transition, including
metadata and full before/after tokens. Reapplication adds no independent evidence.
There is no cache publication, format migration or new persistent state.

## Evidence and limits

Retain the old double-reconstruction implementation in test builds. A 1,024-case
typed mixed-prefix property and deterministic all-six-type transitions compare
schema, allocation catalog, facts, snapshots and subsequent allocation reports,
through keyed/unkeyed writes and reopen. Five semantic tampering cases alter the
report and matching event rows together; both paths reject the invalid log.
Independent stateful domain tests exercise the optimized public path too.

The manual release harness measures original replay stages, including discarded
evaluation cleanup. It preloads rows and includes remaining assertions/cleanup in
wall time, so it is instrumentation rather than an exact task-query breakdown.
Fresh before/after workloads run separately from builds/tests with independently
asserted answers. Recorded head-query medians improve from 180.394 to 85.531ms
at 1,000 tasks/50 revisions and 89.986 to 43.527ms at 100/250. Full raw outputs
are checked in. These are warm container observations, not a production SLA;
full-history replay and unkeyed report validation remain costs.

Validation: the full local check script passed formatting, all-target Clippy
with warnings denied, 142 tests (forty-two 1,024-case properties), the asserted
Rust workflow and warning-free rustdoc. This includes all fault, isolation,
abrupt recovery, backup and assert_cmd scenarios. The one deliberately ignored
manual profiler passed separately in release mode on the final harness; raw
output records `debug_assertions: false`. Cargo stayed offline and assert_cmd
had local socket permission. Whitespace and local Markdown links passed.
An initial Clippy constant-assert finding in the manual harness was resolved by
recording build mode in its output; a relative-directory invocation was corrected
and the harness now requires an absolute profiling directory.
