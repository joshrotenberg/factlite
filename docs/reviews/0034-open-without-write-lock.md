# 9.3 Open without the write lock: self-review

Reviewed the complete diff. This is a self-review. No independent review or
remote CI ran.

## Change

`Store::open` previously ran its full log validation inside `BEGIN IMMEDIATE`.
It now classifies the file in a deferred transaction. An existing format-2
store is validated there and the transaction commits without writing. An
empty file or a format-1 file switches to IMMEDIATE, classifies again under
the lock, initializes or upgrades as needed, validates, and commits. The WAL
journal-mode step is unchanged and does not need the write lock when the file
is already in WAL mode.

Self-review checked these points:

- Validation reads one consistent snapshot: head, tables, events and intervals
  are all read inside one transaction, as before.
- Initialization races are closed by reclassifying under IMMEDIATE. A second
  process that loses the race finds a current-format file and only validates.
- Unsupported and unrelated files still fail before any change. Classification
  no longer requires `tables.is_empty()` separately, because zero user objects
  implies no tables.
- Read-only opens (`Store::reader`) already used a deferred transaction and are
  unchanged.

## Evidence

- New: `opening_an_existing_store_neither_takes_nor_waits_for_the_write_lock`
  failed before the change (`Busy` after 2.2 s) and passes after it.
- New: six threads opening one new file together agree on one store identity.
- Rewritten: `held_writer_lock_has_bounded_busy_failure` used to assert that
  opening an existing store fails `Busy` under a held lock, which was the
  defect. It now asserts the remaining bounded-`Busy` cases: initializing a new
  file, and committing with a confirmed rollback.
- Manual, on a copy of the clajent session's 2,803-revision store: a pod process
  committed 200 times at 50 ms intervals while another process opened the file
  three times (7.8 to 9.0 s each). There were zero `Busy` errors, and the worst
  commit took 21 ms.

Validation: `scripts/check.sh` passed on macOS 27.2, Rust 1.99.1, with 160 tests
(forty-four 1,024-case properties, one ignored manual profiler), the Rust
workflow and warning-free rustdoc.

## Limitations

- Open time is unchanged and grows with revisions times live state. A process
  still waits for its own validation; only other processes are no longer
  blocked.
- Commits that land during an open's validation are not covered by that
  validation; the next open checks them.
