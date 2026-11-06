# Testing policy

Every semantic item includes deterministic unit tests, property tests where
the state space is large, and public integration tests. Scenario tests exercise
complete transactions and retained views as those surfaces become functional.
Use `assert_cmd` for separate-process CLI scenarios once database commands exist.

`scripts/check.sh` is the standard gate. Property suites explicitly run at least
1,024 cases per property, including numeric bit patterns and randomized batches.
Retain any discovered minimal counterexample as a regression test. Randomized
coverage complements deterministic boundaries; it is not a claim to enumerate
all possible inputs. Do not claim exhaustive proof from a passing suite.

Test independent observable contracts: final-state set semantics, rollback,
historical identity/schema, retry precedence, and public error context. Avoid
testing private spelling or copying the algorithm into an alleged oracle.
Use a small replay/reference evaluator to compare persistence and optimized
paths. Crash, locking, backups, and process behavior need integration evidence.

Repository hosting remains local during this session. Preserve Git history,
reviews, lockfile, and documentation for a later tarball handoff; exclude build
outputs, local toolchains, and transient databases when packaging is requested.

The SQLite fault suite uses test-only hooks and temporary triggers; production
code exposes neither. Its eight write fault locations plus two COMMIT paths
prove transaction rollback/classification, not physical power-loss recovery.
Concurrent-connection tests use barriers and no scheduling sleeps.

Reader isolation tests force a separate WAL writer to commit between head reads
and the first log read using test-only authorizer callbacks. All six public read
surfaces keep A; an operation-boundary test also checks SQL head and log remain
at A even with a deliberately broad log bound. Since 9.1 each read surface is
also forced at two more points: before its SQL snapshot starts (first
`store_info` read, so B is visible and only revision bounds keep A) and after
it starts (first `datoms` read, so B must be invisible).

The CLI's nine assert_cmd scenarios exercise separate processes, complete A/B/
reopen/retry reports, Rust parity, all types and error codes, streams/exit codes,
busy handling, native file paths, and stdin. Each subprocess has a 20-second
timeout. In the managed sandbox, the wait-timeout helper needs local socket
permissions; the gate was run with socket access and Cargo offline. The runnable
documentation example was also executed in a temporary directory with jq.

Historical SQL is compared to reference evaluation in 1,024 typed randomized
small graph cases through predicates, distinct total ordering/limits, later
schema writes, and old-view reopen. Independent integration oracles still run
through the public SQL path. The forced writer suite also commits after TEMP
materialization and before SQL reads; no head projection is involved. TEMP write
failure injection verifies cleanup and reuse without durable state changes.

Process recovery re-enters one exact test in a child and handshakes through pipes,
with a 20-second readiness bound and kill-on-drop cleanup. Cache pressure spills
WAL frames; abrupt termination before COMMIT must retain only the old complete
state. Termination after COMMIT but before returning a report must recover every
original report/ID/key/metadata byte via retry. No production fault controls or
physical power-loss claim. Eight actual CLI processes start behind barriers:
same-key requests allocate once and conditional requests have exactly one winner.

Backup evidence includes a deterministic writer committing B after the source
snapshot A is fixed but before native SQLite copy; destination stays A and
rejects B's future token. Failure and publication-race tests preserve existing
files and clean staging. Read-only connection tests reject both raw main writes
and a private commit attempt. A 1,024-case property preserves all six value
kinds, old schema/facts/events/queries and complete history; a faithful restore
retires all source handles before continuing. CLI checks standalone artifact
bytes and absence of new WAL sidecars after actual read subprocesses.

The release workload example checks independent expected joined rows while
measuring four graph/history configurations on both tmpfs and workspace files.
Setup/assertions are outside measured operations. Custom one-task/one-revision
and small workloads run, six invalid configurations fail, and scratch cleanup
is verified. Benchmarks are separate from test counts and latency is not an SLA;
raw results and environmental limits live in `measurements.md`.

The Rust first-use workflow runs as part of the standard gate, with asserted
graph/condition/query/retry/backup/reopen outcomes in a fresh temporary directory.
The minimal crate documentation example is a compiled and executed rustdoc test.

Mixed stateful traces use a separate domain oracle (nodes, scalar slots, unique
owners, reference targets and sets), not the private evaluator. They predict IDs,
complete reports and sorted event differences before submission. Sixteen command
kinds combine successful and rejected writes, schema evolution, forward cycles,
empty allocations, conditions/fences, exact/changed retries, unkeyed no-ops and
actual handle closure/reopen. Each step checks current and selected historical
views; final reopen checks every basis and original retry report. A deterministic
twenty-command scenario complements 1,024 shrinking traces of up to twelve
commands. Small bounded traces keep failures reproducible; they do not enumerate
every possible sequence or replace the separate concurrency/recovery suites.

Evaluated-state reuse is compared with the original double-reconstruction replay
in 1,024 typed mixed-prefix cases, plus deterministic all-six-type transitions
and five tampering cases. Checks include future allocation behavior. Tampering
changes report/event rows together while retaining original canonical evidence.
The full gate reruns independent stateful, fault, isolation, crash/recovery,
backup and CLI tests through the optimized public path. A deliberately ignored
manual stage profile is run separately in release mode; its timings are benchmark
evidence, not an additional ordinary semantic test or a latency assertion.

The datom index is compared with full replay at every revision of 1,024 random
mixed histories over all six types and both cardinalities, including conflicting
batches, retract/reassert cycles and reopen. The typed replay-parity trace also
compares indexed state at each basis. Deterministic tests cover separate
intervals for a reasserted fact, format-1 upgrade and its rollback on failure,
six interval tampering forms on writable and read-only open, and two interval
write faults in the rollback suite.

The pod's translation layer has unit tests for each value type in both
directions, rejection of every unsupported Clojure type, strict transaction,
query and snapshot maps, error data, and a 1,024-case value round-trip property.
A parity test checks that a pod request and the equivalent CLI JSON decode to
equal requests with identical canonical bytes. `crates/factlite-pod/test.bb`
runs 59 checks under real babashka over stdio and socket transports. The
contention test runs four pod processes, one on the socket transport, against
one file: keyed creations land exactly once, conditional increments serialize,
and the final revision count is exact. Set `FACTLITE_POD_WORKERS` and
`FACTLITE_POD_ITERATIONS` for a heavier run. These tests skip when `bb` is not
on PATH unless `REQUIRE_BB` is set.

Open's lock scope is tested by holding `BEGIN IMMEDIATE` on another connection:
opening an existing store must succeed in under a second and read normally,
while initializing a new file and committing fail `Busy` within a bound with a
confirmed rollback. Six threads opening one new file together must agree on one
store identity.

Incremental transitions are compared with the retained full-state transitions in
1,024 random sequences of up to fifteen requests. The requests cover cardinality
one and many, unique scalar and reference attributes, temporary labels,
fabricated identities, wrong types, schema redeclaration and conflicts,
expectations and stale head fences. Successful requests must produce identical
states (including next allocation) and reports. Failing requests must produce
identical error codes, contexts and messages; every evaluation error code
occurs. Each successful report, and five deterministic damaged variants, are
applied through both report paths, which must agree on acceptance and state. A
deterministic case checks the unique-violation choice among several
simultaneous violations. Replay parity and the tampering suite compare
incremental replay with full-state replay.
