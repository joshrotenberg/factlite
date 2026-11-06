# 9.4 Incremental replay validation: self-review

Reviewed the complete diff. This is a self-review. No independent review or
remote CI ran.

## Change

`Model` keeps its fact set private behind `facts()`, with lazily built indexes
(values per entity/attribute, owners per unique attribute/value) maintained by
`insert_fact` and `remove_fact`. `evaluate_owned` and `apply_report_owned`
transition in place. Expectations read the value index. Normalization stages
the catalog in place and checks concrete identities at the pre-head revision.
It removes replaced cardinality-one values via the index and computes the
effective delta from touched facts. Uniqueness and report validation check
asserted facts only. Commit and replay use the owned forms. Replay is generic
over an `Incremental` or test-only `Reference` transition strategy.

Self-review checked these points:

- Check order in `evaluate` is unchanged, so the first failing check, and so the
  returned error, is the same: structural validation, head fence,
  expectations, staging, per-change resolution, conflicts, one-valued
  conflicts, uniqueness. Differential tests compare code, context and message.
- Pre-head identity checks: before, normalization checked concrete IDs
  against the unstaged catalog. Now `require_known_at(id, pre)` checks them
  against the staged catalog at the pre-head revision, which excludes this
  request's allocations exactly as before. A fabricated token guessing a new
  allocation still fails `UnknownEntity`.
- The induction premise holds on every production path. Replay starts from
  `Model::empty`. Commit starts from `index::model_at`, whose state equals
  validated replay at open (9.1) and is maintained by validated commits.
- The unique-violation tie-break keeps the least (attribute, value) pair and
  ignores repeats of the same pair, matching the former BTreeMap scan and
  `min_by` over canonical value order.
- Test-only setters reset the indexes, so tests that build states directly
  cannot leave stale indexes.

## Evidence

- New 1,024-case differential property over random request sequences:
  identical states and reports on success, and identical errors on failure.
  Every evaluation error code occurred. In a counted run, 1,211 successful
  transitions changed facts. Damaged reports are applied through both report
  paths, and acceptance and resulting states always agreed. The counting was
  temporary instrumentation and is not committed.
- New deterministic unique-violation selection test with simultaneous
  violations on two attributes.
- Existing: the typed replay-parity property and trace now compare incremental
  replay against full-state replay, and the five tampering cases must fail on
  both. Index parity, stateful traces, fault, isolation, recovery, backup, CLI
  and pod suites all pass unchanged.
- Measurements: see `measurements.md`. On a copy of the clajent store, open
  fell from about 7.5 s to 0.14 s.

Validation: `scripts/check.sh` passed on macOS 27.2, Rust 1.99.1, with 162 tests
(forty-five 1,024-case properties, one ignored manual profiler), the Rust
workflow and warning-free rustdoc.

## Limitations

- Open remains linear in retained revisions; no checkpoint exists.
- Commits and reads still load the full state at head and are linear in live
  state. The commit path also builds the indexes for that state once per commit.
- Correctness depends on the induction premise. A model built directly from
  invalid facts would not be fully revalidated; only tests do that, and they
  reset the indexes.
