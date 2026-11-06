# 9.1 Temporal datom index: self-review

Reviewed the complete production, test, benchmark and documentation diff.
This is a self-review. No independent review or remote CI ran.

## Change

Format 2 adds a derived `datoms` table with one `[asserted, retracted)` revision
interval per effective assertion. `persist_report` writes intervals inside the
commit transaction. Reads and commit evaluation load the state at R from
revision-bounded schema/entity events and visible intervals, in the operation's
single SQLite transaction, instead of replaying the log. Every open still runs
full replay validation, then requires stored intervals to equal intervals
derived from fact events, then requires indexed head state to equal replayed
head state. Writable opens upgrade format-1 files in their open transaction.

Self-review checked these points:

- Interval derivation needs only fact events. Retained events are already
  validated against reports and replay, so equal stored and derived intervals
  imply the indexed state at every R equals the replayed state. Property tests
  check that claim directly at every revision.
- Visibility `asserted <= R AND (retracted IS NULL OR retracted > R)` matches
  set semantics: an event at revision R is applied in the state at R. Reasserting
  a retracted fact opens a new interval rather than reopening the old one.
- The catalog's next allocation is the maximum retained allocation plus one,
  matching contiguous allocation from one, including exhaustion at `u64::MAX`.
  Parity compares it explicitly, not only visible state.
- Basis identity and future-revision checks are unchanged and run first.
- Retry lookup still precedes state loading in commit. Failure at either
  interval write stage rolls back reports, events, intervals and head.
- Upgrade failure leaves the format-1 file byte-for-byte untouched by SQLite
  rollback. A test checks user version and table absence after a failed upgrade.

## Evidence

- 1,024-case property: random mixed histories over all six types and both
  cardinalities, three operations per batch (conflicting batches abort), with
  indexed/replayed state compared at every revision after reopen.
- The existing typed replay-parity property and deterministic trace now also
  compare indexed state at each basis.
- Deterministic cases: separate intervals and historical views for a reasserted
  fact, format-1 upgrade with every prior view preserved, rollback of a failed
  upgrade, and six interval tampering forms. Those forms are a deleted row, a
  reopened closed interval, a closed open interval, a changed value, a shifted
  assertion and an extra row. Each one fails both writable and read-only open
  with `Storage`.
- Two interval write faults were added to the rollback suite, and the suite
  checks that no interval rows remain.
- The forced-writer isolation test now covers 15 interleavings: each read surface
  before its SQL snapshot starts (B visible, revision bounds keep A) and after
  the snapshot starts, plus the previous log and TEMP relation points.
- Measurements: see `measurements.md`. At 1,000 tasks/50 revisions on an Apple
  M4 Pro, head-query median fell from 41.088 to 4.922ms and no-op commit from
  45.124 to 2.901ms. Read-only open rose 5–28% across the four workloads.

Validation: `scripts/check.sh` passed on macOS 27.2, Rust 1.99.1. That covers
formatting, all-target Clippy with warnings denied, 147 tests (forty-three
1,024-case properties, one ignored manual profiler), the asserted Rust workflow
and warning-free rustdoc.

The CLI non-UTF-8 path round trip failed on APFS before this change, because
APFS rejects such names. It now runs only where a probe file with a non-UTF-8 name
can be created. Argument rejection still runs everywhere. This is a separate
commit.

## Limitations

- Integrity is verified at open. Reads after open no longer revalidate the log,
  so external SQL writes made while a store is open go undetected until the next
  open. Such writes are unsupported.
- Open remains proportional to retained history, and slightly slower than before.
  CLI commands open per process, so they still pay full replay.
- The open-time table check does not check indexes. A dropped `datoms_open`
  partial index removes a backstop only; derived-interval comparison still
  detects inconsistent rows at open.
- Queries still copy the visible state into a TEMP relation, and facts/pull scan
  the loaded state. Direct revision-bounded SQL is item 9.2.
- Entity and attribute columns are canonical text. Numeric keys could shrink rows
  and indexes, but no measurement has justified them yet.
