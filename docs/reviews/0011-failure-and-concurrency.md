# Review: item 2.4, failures and concurrent connections

- **Branch:** `storage/failure-and-concurrency`
- **Reviewer:** Codex self-review.

Reviewed all fault fixtures, concurrency cases, feature configuration, and diff.
Temporary triggers inject failures before report/schema/entity/fact/head writes,
including late array inserts after partial staged data. Each proves unchanged
head/original report and empty new events, then removes the fault and commits
successfully. Deferred constraints force a COMMIT error with explicit rollback;
a commit hook exercises conservative Unknown classification after transaction
termination. Hooks are a dev dependency and fixtures remain cfg(test).

Barrier-synchronized separate connections prove only one stale-condition update
succeeds and twelve fresh no-change commits receive contiguous unique revisions.
Live-store tests also prove unrelated changes pass attribute expectations while
failing exact-head fences, and bounded Busy failures confirm no write and permit
retry after lock release. The first lint run found ignored hook setup results;
those now unwrap so test setup failures cannot pass silently.

`scripts/check.sh` passed: sixty-seven tests, twenty-two 1,024-case properties,
fault/concurrency/integration/scenario suites, formatting, Clippy, and docs. No
unresolved findings. These are multiple connections/threads, not process-kill,
power-loss, or IO/VFS experiments; those remain the hardening/CLI stages.
