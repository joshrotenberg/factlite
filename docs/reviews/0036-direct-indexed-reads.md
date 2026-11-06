# 9.2 Direct indexed reads: self-review

Reviewed the complete diff. This is a self-review. No independent review or
remote CI ran.

## Change

`Db` reads run `index::check_basis` and then revision-bounded SQL: schema
events, entity events, and `datoms` rows visible at R. The query compiler
targets `main.datoms` with a visibility condition per pattern instead of
building a TEMP relation. Format 3 adds `datoms_avet`. Open validates the datom
index set and upgrades formats 1 and 2 under the write lock.

Self-review checked these points:

- Visibility matches replay. An interval `[asserted, retracted)` is visible at
  R exactly when the fact is in the replayed state at R, and at most one
  interval per fact is visible at any R.
- Typed equality in SQL. Within one attribute every value blob has the same
  type, and the query validator rejects incompatible variable domains, so blob
  equality in joins is typed equality. Mixed-type predicates still compile to
  constant true/false as before. Entity-to-reference joins compare identity
  text with UTF-8 reference blobs through explicit casts.
- Ordering. Selected expressions are BLOBs, as in the TEMP design, so
  DISTINCT/ORDER BY behave as before. `facts`/`pull` still sort in Rust.
- Isolation. Every read checks its basis and reads in one deferred
  transaction, and the forced-writer test injects a commit before the snapshot
  and at each table the read touches afterwards.
- The zero-pattern query never references the basis parameter, so it is
  wrapped to keep the binding count fixed.

## Evidence

- New 1,024-case property: SQL reads equal full log replay at every revision,
  for every filter shape and every pull.
- The typed query property (1,024 cases) now includes real later changes, and
  query results at A must be unchanged after them and after reopen.
- New tests: format-2 upgrade, and a missing index failing writable and
  read-only opens. The format-1 upgrade test now expects format 3.
- Updated tests: the compiled-SQL snapshot; a denied `datoms` read replaces the
  TEMP insert failure; the isolation interleavings.
- Measurements: see `measurements.md`.

Validation: `scripts/check.sh` passed on macOS 27.2, Rust 1.99.1, with 165 tests
(forty-six 1,024-case properties, one ignored manual profiler), the Rust
workflow and warning-free rustdoc.

## Limitations

- No `ANALYZE` statistics are kept; plans rely on SQLite's defaults, which chose
  the primary key and attribute/value index for the measured joins.
- Unfiltered `facts` and value-only filters scan all intervals.
- Commits still load the full state at head (item 9.5).
