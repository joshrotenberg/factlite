# Review 0052: permanence and encryption guidance (12.4)

Branch `docs/permanence-encryption`, PR #17. Self-review plus one independent
Codex subagent review of the diff, run with the kit subagent tooling. No Claude
subagent review ran for this documentation-only item. No human reviewer and no
remote CI ran.

## Scope

Documentation only. New `docs/permanence.md`; short sections or links in the
README, `docs/rust.md`, the pod README, `docs/cli.md` and `docs/backup.md`;
status, plan, roadmap and decision brief 06 updated. No code or behavior
changed.

## Self-review

Claims checked against the code before the diff review:

- Canonical requests are stored only for keyed requests (`store.rs` table
  check, `log.rs` insert).
- Error codes and the contexts that carry values (`error.rs`): the first draft
  named `UniqueConflict`; the code is `UniqueViolation`. Fixed.
- Reports carry no commit time; the first draft said commit times stay
  readable. Fixed.
- No `secure_delete` or `VACUUM` anywhere in `crates/factlite/src`; WAL is set
  in `store.rs`, and backups switch to DELETE journal mode in `backup.rs`.

## Codex diff review

Seven findings, all checked against the code and adopted:

- **"A failed transaction stores nothing" ignored unknown outcomes.** `store.rs`
  returns `CommitOutcome::Unknown` when COMMIT fails after autocommit resumes.
  Reworded: rejected and confirmed-rolled-back requests add no records; an
  unknown outcome may have committed.
- **Sanitized export did not say to filter.** Now an application migration that
  leaves out the data, remaps references to new entity IDs and destroys the old
  file and copies.
- **Retracting a fresh encryption "has no effect".** The stored fact stays, but
  the request still commits a revision and a keyed request retains the
  ciphertext. Reworded.
- **Re-encryption does not always fail `IdempotencyConflict`.** Only when the
  original committed under that key and the ciphertext differs. Reworded.
- **WAL is not always on.** Backups use DELETE journal mode. Reworded, and a
  truncating checkpoint is described as effective only when it completes.
- **`secure_delete` overstated.** It affects later deletions, not pages freed
  earlier; its default depends on the build. Reworded, scoped to a future tool
  that removes rows in place.
- **Backups do not copy every value the source ever committed.** They copy
  through their captured revision. Reworded in `backup.md` and the summaries.

It also suggested stating that expectations compare the exact pre-state set
and uniqueness checks final-state ownership. Added.

## Validation

`REQUIRE_BB=1 ./scripts/check.sh` passes locally (220 tests). GitHub Actions is
unavailable for this account this month.

## Limitations

- The encryption guidance is not exercised by a test. The behaviors it relies
  on (byte equality, uniqueness, expectations, idempotency conflicts) are
  covered by existing tests of `bytes` values and keyed retries.
- SQLite behavior (`secure_delete`, `VACUUM`, checkpoints) is described from
  SQLite's documentation, not tested here.
