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
