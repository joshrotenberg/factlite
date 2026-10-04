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
at A even with a deliberately broad log bound. No current projection exists yet;
introducing one requires its own forced head-check/read race regression.

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
