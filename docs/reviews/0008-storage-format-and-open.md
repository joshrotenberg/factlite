# Review: item 2.1, storage format and open

- **Branch:** `storage/format-and-open`
- **Reviewer:** Codex self-review.

Reviewed connection ownership, bootstrap transaction, DDL, format checks,
revision encoding, errors, dependencies/lockfile, and all tests. SQLite-safe
locking serializes initialization; schema, identity, and head are committed
together. Persisted IDs survive reopen and separate files have separate bindings.

Review corrections before merge:

- Set FULL sync before bootstrap writes, not just after WAL setup.
- Empty-file detection considers every user schema object, including views.
- Use GLOB rather than LIKE so underscore in `sqlite_*` is literal and tables
  such as `sqliteXuser` are not mistaken for SQLite internals.
- Added held-writer integration coverage for bounded Busy errors, source
  preservation, and unchanged revision-zero state after lock release.

`scripts/check.sh` passed: forty-seven tests, seventeen 1,024-case properties,
six storage integrations, existing semantic suites, formatting, Clippy, and docs.
Reviewed staged whitespace and complete diff. No unresolved findings. Current
open consistency checks are structural; full semantic replay validation follows
in item 2.2. No public commit/read-view or CLI database API is claimed yet.
