# Review: item 2.3, public atomic commits

- **Branch:** `storage/atomic-commits`
- **Reviewer:** Codex self-review.

Reviewed commit control flow, explicit rollback/outcome handling, error source
retention, public integration cases, and complete diff. All state-dependent
evaluation occurs after BEGIN IMMEDIATE and basis capture. Reports/events/head
persist in that transaction and no mutable model is published before COMMIT.
Removed the temporary model/persistence lint expectations after integration.

Inspected rusqlite 0.40.2 transaction source: consuming commit invokes drop-time
cleanup that ignores rollback errors. The implementation executes COMMIT with
the guard retained, then inspects transaction state or explicitly rolls back.
This avoids claiming confirmed rollback merely because a guard was dropped.

`scripts/check.sh` passed: sixty tests, twenty-two 1,024-case properties,
public create/replace/no-op/reopen, failed-constraint rollback, key rejection,
head fencing, all stored value types, existing semantic/replay suites, formatting,
Clippy, and docs. No unresolved findings. Fault injection and competing writers
are item 2.4; physical crash/power-loss outcomes still require later evidence.
