# 12.1 Read-only structural open and verify: reviews

## Change

- **Library:** `Validation::{Full, Structural}` on `StoreOptions`; a
  structural path in `Store::reader`; `Store::verify` and `Db::verify`.
- **Read hardening:** schema-type checks on decoded facts, a type-tag condition
  in compiled queries, a retained-revision check in `index::check_basis`, a
  contiguity check in `log::reports_between`, and a duplicate check in `pull`.
- **CLI:** `--validation` on read commands, and `verify`.
- **Pod:** `:validation` on `open`, and `verify`.

## Codex second opinion on the plan

Adopted before implementing:

- **Structural reads could panic.** A mistyped row reached aggregation through
  the reference evaluator. Fixed with schema-type checks on decoded facts.
- **Verify must recheck format markers and keep error codes.**
- **A snapshot inside a gap in history could be read.** Fixed with the
  retained-revision check.
- **Extra tests:** pinned views, post-open tampering, an empty store, and a
  refused writable open that creates no file.

## Subagent reviews

Two independent subagent reviews of the implementation ran through kit: Claude
(review-pr recipe, PR #14) and Codex (review-diff profile). Findings were
confirmed in the code and with new tests.

Fixed:

- **SQL queries decoded mistyped rows as the schema type (both).** A
  same-width row could be silently reinterpreted. Compiled queries now match
  on the stored type tag. Tested with an inst-tagged row on an i64 attribute.
- **Report-backed history and transactions skipped the type check (both).**
  Facts decoded from reports are now checked.
- **Interior gaps were silently omitted from transactions and history
  (Claude).** Report reads now require contiguous revisions. A trailing-gap
  check was tried and dropped: reads already check that their basis revision
  exists, and an isolation test deliberately reads past a snapshot's head.
- **The schema was reloaded on every fact read, including on commits
  (Codex).** It is now loaded once per operation.
- **`pull` hid duplicated one-valued rows (Claude).** They now fail `Storage`.
- **Verify blocks clones of its handle (Claude).** Documented.
- **Missing tests (Claude).** Added structural reads over a tampered report
  and gaps, the SQL path, duplicates, a repeated `--validation` flag, `verify`
  on a missing file, and that a refused pod open creates no file.

Accepted as limits:

- Neither structural nor full validation checks table DDL beyond names.
- Wrong answers from a corrupted index remain possible on a structural view
  until `verify`. That is the decided contract, and it is documented.
- No test forces a concurrent commit during `verify`. Verify uses one read
  transaction, as every read operation does, and the existing isolation suite
  covers that pattern.

## Self-review and validation

Self-review: confirmed that no structural `Db` can reach `Store::commit`. It
holds a private, physically read-only connection, and `Store::open_with`
refuses structural validation before opening.

Validation, local only (GitHub Actions is unavailable for this account, and
the user directed that the local gate stand in for CI): `REQUIRE_BB=1
scripts/check.sh` passed with 197 tests, including forty-nine 1,024-case
properties plus the new one, and the pod's babashka tests.
