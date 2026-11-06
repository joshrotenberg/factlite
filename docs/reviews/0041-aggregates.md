# 11.4 Aggregates: self-review

Reviewed the complete diff. This is a self-review. No independent review or
remote CI ran.

## Change

The query AST adds `with` and `aggregates`; validation covers bound variables,
`with` without aggregates or overlapping the selection, `sum` domains, column
name collisions and order fields. `query_aggregate` owns tuple columns,
grouping, the five functions, ordering and limits. The reference evaluator
projects tuple columns and calls it. The SQL path runs the existing compiler on
the derived tuple query and calls it. Wire, CLI documentation and pod syntax
follow.

Self-review checked these points:

- Plain queries are unchanged: same projection, SQL, order and limit; the
  reference ordering moved into a shared helper with identical comparisons.
- Groups are never empty, so min/max/sum always have input; zero tuples give no
  groups and so no rows.
- Sum overflow and non-finite totals fail with the same message on both paths.
  F64 sorting by canonical order makes the result depend only on the tuple
  multiset.
- Results without an order have unspecified row order, as before. Tests
  compare such results as sets.
- The pod reorders columns to `:select` order and maps `(fn ?var)` in `:order`
  to the same derived name.

## Evidence

- Scenarios: per-group results with and without `with`; global aggregates;
  ordering by an aggregate with a limit; historical views; zero limit; no
  matches; min/max over strings and references; i64 and f64 overflow; nine
  validation failures.
- Independent-oracle property (1,024 cases): grouped count, count-distinct,
  sum, min and max over random i64 or f64 runs, with and without `with`,
  computed in the test without the aggregation module.
- Parity property (1,024 cases): the typed-graph join aggregated per entity,
  over all six types and sum for numeric ones, through SQL and the reference.
  Results must be identical, or failures identical. This holds at view A,
  after later changes, and after reopen.
- The wire round-trip property now includes random aggregates and `with` lists.
- Pod unit test for naming, ordering and unknown functions, and seven new
  babashka checks per transport.

Validation: `scripts/check.sh` passed on macOS 27.2, Rust 1.99.1, with 179
tests (forty-nine 1,024-case properties), the Rust workflow and warning-free
rustdoc.

## Limitations

- No average; callers can divide a sum by a count.
- Aggregation runs in Rust after SQL returns the distinct tuples, so memory is
  proportional to the distinct tuple count.
- Aggregates cannot be used in predicates (no HAVING).
