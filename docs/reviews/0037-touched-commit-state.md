# 9.5 Commits load only touched state: self-review

Reviewed the complete diff. This is a self-review. No independent review or
remote CI ran.

## Change

`Store::commit` builds its model with `index::model_for_request` instead of
`index::model_at`. `Catalog::partial` carries the true next allocation, read
from the greatest identity. The evaluator is unchanged.

Self-review walked every state access in `evaluate_owned` and its callees:

- Schema reads and staging conflicts use the complete schema, which is loaded.
- `require_known` and `require_known_at` are called only for expectation
  entities and values, change entities and reference values. All are named
  identities, loaded with creation revisions; unknown and foreign identities
  have no entity event and stay unknown.
- Value-index reads happen for expectations and cardinality-one replacements.
  Both are touched entity/attribute pairs, whose visible values are all loaded.
  A temporary entity has no prior facts.
- Fact membership is tested only for retracted, replaced and asserted facts,
  all within loaded pairs.
- Owner counts are read only for asserted unique values, whose owners are all
  loaded through the attribute/value index. Owners loaded for other values
  through entity pairs may be partial, but they are never consulted.
- Allocation uses the stored next allocation, not the partial map.

## Evidence

- New 1,024-case property: random request sequences evaluated on touched and
  full state and actually committed, with identical reports and errors.
- New allocation test across empty, single and multiple allocations.
- All existing commit, retry, fault, contention, recovery, stateful, CLI and pod
  tests pass on the new path.
- Measurements: see `measurements.md`.

Validation: `scripts/check.sh` passed on macOS 27.2, Rust 1.99.1, with 167 tests
(forty-seven 1,024-case properties, one ignored manual profiler), the Rust
workflow and warning-free rustdoc.

## Limitations

- The argument depends on the evaluator reading state only through the paths
  above. A future evaluator change that reads other state would need the loader
  updated; the differential property is the guard.
- Open remains linear in revisions plus live state.
