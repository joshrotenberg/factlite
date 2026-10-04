# Item 5.1 self-review

Reviewed the complete public AST, historical validator, and integration-test diff.

- Private validated names and typed terms preserve invariants. Fixed attributes
  resolve against Db's basis. Entity/Ref unification and exact variable domains
  reject scalar entity joins without coercion.
- Parameter presence/type, bound comparisons/projections, unique selected names,
  selected-only ordering, and same-type ranges validate before any execution.
  Equality across distinct types retains exact inequality semantics.
- Deterministic tests cover historical schema after later commits/reopen, ref joins,
  wrong/undeclared/missing/unused inputs, constants, foreign reference syntax,
  unbound/duplicate/unselected names, zero/full-u64 limits, and exact Unicode names.
- Two 1,024-case properties span all six domains, pattern permutations, and all
  comparisons. One initial fixture selected an entity variable after replacing
  it with a constant; corrected the fixture to test the intended domain rule.
- Formatting, Clippy with denied warnings, 112 tests (35 properties), documentation
  with denied warnings, and whitespace passed, including all CLI scenarios.

No findings remain. Self-review only; remote CI has not run. This item exposes
validation without claiming execution. Matching, wire parity, and bound historical
SQL are the next three increments.
