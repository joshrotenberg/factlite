# Concurrent first open retries the switch to WAL: reviews

## Defect

CI on PR #5, which changed only docs, failed in
`concurrent_first_opens_initialize_one_store_identity` (item 9.3): `Store::open`
returned `Busy`. Labeling each SQLite step in `open` located the failure at
`PRAGMA journal_mode=WAL` on a newly initialized file. A scratch run with 32
threads failed 2 to 6 opens per 1,600.

## Fix

`enable_wal` retries the switch until the pragma reports `wal`, within the
configured busy timeout overall. Each attempt's own busy wait is limited to
the remaining budget, and the original timeout is restored afterwards. The
connection-setup pragmas and the transaction steps of `open` are labeled in
errors.

## Subagent reviews

Two independent subagent reviews ran through kit: Claude (review-pr recipe,
PR #6) and Codex (review-diff profile). They had the diff and the defect
description. Findings were checked against the code.

Fixed:

- **Retry could exceed the timeout (both).** Each attempt kept SQLite's full
  busy timeout, so the worst case was about twice the budget. Attempts now get
  only the remaining budget, and the deadline is checked before sleeping.
- **A failed switch could look like corruption (Claude).** SQLite can report
  the old mode instead of returning `Busy`. The loop now reads the mode the
  pragma returns and retries while it is not `wal`. The separate,
  non-retried verification query is gone.
- **No deterministic tests (both).** Added tests that hold a read lock on a
  rollback-journal store:
  - zero and 200 ms timeouts fail `Busy` with the `open: enable WAL` label,
    within budget (and, for 200 ms, after at least the budget);
  - open succeeds once the lock is released mid-wait.
- **Stress test cost and diagnostics (both).** Reduced to 10 rounds of 32
  threads, and failures now report the round and the labeled error.
- **Unlabeled steps (both).** The connection-setup pragmas are now labeled
  `open: configure connection`.

Rejected:

- **The read-only open has the same hazard (Claude).** It cannot switch
  journal modes, which needs write access, so it has no read-to-write promotion.
  Codex reached the same conclusion from the code.
- **The deferred validation read could get an unhandled `Busy` (Claude,
  stated as plausible).** Ordinary read locks use the busy handler. No failure
  occurred in 19,200 stressed opens after the first fix, and the stress test
  remains as a guard.

## Self-review and validation

Self-review: the retry holds no locks while sleeping, `pause` grows from 1 ms
to 50 ms, and a zero timeout makes exactly one attempt. Validation:

- The original 20-round stress test failed 7 of 10 release runs before the
  fix and none after.
- The `store_open` suite passed 20 of 20 repeated debug runs.
- `scripts/check.sh` passed locally with 183 tests.
