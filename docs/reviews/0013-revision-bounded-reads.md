# Item 3.2 self-review

Reviewed the complete API, bounded SQL helper, and integration-test diff.

- Every public read delegates to Db's one-transaction boundary. Both history
  bounds are bound SQL parameters encoded with unsigned revision ordering.
- Filters validate historical schema/types. Pull distinguishes unknown identities
  from empty allocated entities and omits absent fields. Replayed state already
  enforces cardinality-one before projecting a single value.
- Tests cover all six types and ordering, duplicate facts/projection names,
  negative zero, exact i64/f64 distinctions, future attributes/identities, revision
  zero, omitted fields, metadata/noops, filtered ordinals, invalid bounds, reopen.
- A 1,024-case property compares pinned facts, pull, event effects, transaction
  bounds, and reopened results against independently maintained integer sets.
- Formatting, Clippy with denied warnings, 74 tests (24 properties at 1,024
  cases), documentation with denied warnings, and whitespace passed.

No findings remain. Self-review only; remote CI has not run. Replay is the
correctness-first implementation; these results make no performance claim.
The complete design fixture and forced isolation race are the next two items.
