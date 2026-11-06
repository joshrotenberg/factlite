# Review 0050: query negation (12.2)

Branch `feat/query-negation`, PR #15. Self-review plus independent Claude and
Codex subagent reviews, run with the kit subagent tooling. No human reviewer
and no remote CI ran.

## Plan second opinion (Codex)

Codex reviewed the plan before implementation. It agreed with scoped
validation, a correlated `NOT EXISTS` with per-alias visibility, and filtering
before projection and aggregation. Its points, each checked against the code
and applied:

- comparison folding inside a group must use the group's types;
- each group needs a fresh scope, so sibling groups may reuse a local name;
- a local must be bound by a group pattern somewhere in the group, not by
  textual order, and outer variables left out of a `not-join` list are not
  visible in its predicates;
- an empty join list and unused join variables are valid;
- correlation must reuse the outer binding's TEXT and BLOB forms;
- predicate-only groups need `SELECT 1 WHERE ...` without `FROM`;
- group patterns and predicates count toward the SQL budget, and the fallback
  needs its own test;
- wire tests must distinguish an absent `join` from `"join":[]`, and pod tests
  must reject malformed clauses.

It noted that rejecting shadowing tightens the brief's wording. That choice is
recorded in `decisions.md`.

## Diff reviews

Neither review found a correctness bug in the library, wire, pod or CLI code.
Findings and how each was handled:

- **Fallback test passed without the fallback** (both). A 32-pattern group runs
  fine in SQL, so the test could not tell which path ran. Fixed: the group now
  has 65 patterns, which SQLite cannot join (66 tables). A mutation that drops
  group patterns from the budget now fails the test with "at most 64 tables in
  a join"; this was run and observed.
- **Reopen check did not reopen** (Codex). Fixed: the test drops every handle
  and calls `Store::open` on the path before querying at A.
- **Parity property gaps** (Claude). Added a group that uses a variable bound at
  an entity position outside at a value position inside, and the reverse.
  Limits now range over 1 to 7, since limit 0 returns before evaluation and is
  covered by the integration tests. Retractions are deduplicated.
- **Pod join names untested without babashka** (Claude). Added a unit assertion
  that `(not-join [?run] ...)` decodes to join `run`.
- **Stale status** (Claude). `status.md` updated.
- **`Factlite-Design.md` lists negative patterns as out of scope** (Claude).
  Not changed: `AGENTS.md` keeps the supplied design as historical reference.
- **Predicate budget unobserved** (Claude). Not fixed. 129 conjunctive
  predicates run in SQL either way, so no result reveals the path. Recorded as
  a limitation in `testing.md` and `status.md`.

## Self-review

- `Query::types` returns only outer variables, so group locals cannot reach
  selection, `with`, aggregates or order; each fails as an unbound column or
  variable.
- `negation_scope` is called once in validation and again in compilation with
  the same inputs, so the compiler never sees a scope that validation did not
  accept.
- The reference evaluator starts a `not-join` from the join variables alone,
  and a `not` from the whole binding, matching the SQL correlation.
- Encoding omits `not` when empty, so existing envelopes are byte-identical.

## Validation

`REQUIRE_BB=1 ./scripts/check.sh` (fmt, clippy with `-D warnings`, tests,
docs, babashka pod tests) passed locally on macOS: 208 tests. GitHub Actions
did not run, because the account cannot start jobs this month.
