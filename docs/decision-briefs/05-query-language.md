# Decision brief: query language extensions (roadmap item 5)

Status: decided 2026-11-06 (see Decision). `not` and `not-join` are
implemented (plan item 12.2); the rest is not.

## The question

Which query extensions should Factlite take on, in what order, and with what
exact semantics? The design deferred all of them because each one changes
correctness commitments. The candidates are:

- negation (`not`);
- disjunction (`or`);
- predicates on aggregate results (HAVING);
- `avg`;
- rules and recursion;
- nested pull.

## What exists today

A query (`crates/factlite/src/query.rs`) is conjunctive patterns, typed
comparison predicates, selection, `with` and aggregates (item 11.4), ordering
and a limit. It is validated against the view's historical schema before any
data is read.

Queries run as SQL over datom intervals visible at the view's revision
(`query_sql.rs`). Parity tests also run them through the reference evaluator
over replayed state (`query_eval.rs`), and a 1,024-case property requires the
two paths to agree on all eight value types. Not every query test runs both.

Aggregates are computed in Rust over the distinct tuples that SQL returns
(`query_aggregate.rs`). Missing facts simply fail to match: there is no way to
ask for something that is absent.

## Evidence from use

The clajent kit's trial queries
(`dc40beb0-528c-4e3d-bd69-b11daaa58dca/scratchpad/fl/queries.clj` in the clajent
session, outside this repository) used:

- joined filters with a time range;
- one-hop follow-ups through a `:run/follows` reference;
- a status history;
- the change feed;
- lookups by key;
- string prefix ranges;
- cost per recipe, now an aggregate.

Two needs came up that the language cannot express:

- **Absence.** "Runs with no `:run/outcome`", or "runs still running", is not
  expressible directly. The kit checked "none left running" by counting.
- **Chains.** A follow-up chain of any length takes one query per hop.

No request has asked for `or`, HAVING or nested pull.

## Candidates and their semantics

### Negation: `not`

**Proposed form.** `not` holds a group of patterns and predicates. A result
survives only if no extension of its bindings satisfies the group, evaluated
at the view's revision.

**Variable rules.**

- In `not`, every variable must also be bound by the positive patterns (the
  "safe" rule). `not [?r :run/outcome "failed"]` excludes runs whose outcome is
  `"failed"`.
- In `not-join [?r ...]`, the named variables must be bound outside. Every
  other variable is local: it must be bound by a positive pattern inside the
  group, and it cannot appear in selection, `with`, aggregates or outer
  predicates. Validation needs lexical scopes for this.

**Absence semantics.**

- Absence needs `not-join`:
  `(not-join [?r] [?r :run/outcome ?o])` means `?r` has no visible fact of that
  attribute, whatever its value. A plain `not` with an unbound `?o` is a
  validation error.
- An attribute unknown at the view's revision is an error
  (`UnknownAttribute`), consistent with patterns. It is not "trivially
  absent". This holds even when the positive patterns match nothing and with
  `limit` 0, because validation runs first.

**Implementation.** In SQL, a correlated `NOT EXISTS`. Every alias inside it
carries the same visibility condition
(`asserted <= R AND (retracted IS NULL OR retracted > R)`), and typed
comparisons stay as they are. The reference evaluator filters bindings that
have no matching extension. Tests must cover facts asserted and retracted after
the view's revision.

**Cost:** medium. **Value:** high; absence is the most visible gap.

### Average: `avg`

**Result type.** `avg` gives an F64 for I64 and F64 variables, over the same
distinct tuples as the other aggregates.

**Rounding.**

- I64: sum exactly in i128 and divide as an exact rational, rounding once to
  the nearest f64. Converting the sum to f64 before dividing would round twice.
- F64: sum in ascending order, then divide. A non-finite running total is an
  error, as for `sum`. So `[MAX, MAX]` fails even though its mean is
  representable.

**Grouping and empty input.** Grouping and `with` follow the existing aggregate
semantics, and an empty input gives no rows.

**Cost:** small. **Value:** modest. Dividing `sum` by `count` already works,
except when the I64 sum overflows.

### Predicates on aggregate results: HAVING

**Proposed form.** Predicates that name aggregate columns, applied after
grouping and before ordering and the limit.

**Typing.** Count columns are I64, `sum`, `min` and `max` keep the variable's
type, and `avg` is F64, so the existing typed comparison rules apply unchanged.

**Still to define.**

- Scope: which columns HAVING may name (selected and aggregate columns only),
  name collisions, parameters, and which error wins first.
- HAVING must be removed from the tuple query that SQL runs.
- Testing: SQL and the reference evaluator share the aggregation code, so
  parity cannot check HAVING or the arithmetic. These need an independent
  oracle, as the 11.4 aggregate property uses.

**Cost:** small. **Value:** modest.

### Disjunction: `or`

**Proposed form.** `or` holds two or more branches, each a group of patterns
and predicates. Results are the union of branch matches, deduplicated by set
semantics.

**Variable rules.**

- Every branch must bind the same variables with the same types, or use
  `or-join` to name the shared ones, with the other variables local to each
  branch. Otherwise a variable would be bound in some results and unbound in
  others, and Factlite has no null.
- Bindings shared by overlapping branches are deduplicated before
  aggregation, so `with` counts are not inflated.

**Implementation.** In SQL, a `UNION` of compiled branches. The reference
evaluator unions branch bindings.

**Cost:** medium. **Value:** low so far; no request yet.

### Rules and recursion

**Proposed form.** Named rules with Datalog-style heads, such as
`(follows-chain ?r ?root)`, defined in the query, possibly recursive.

**Semantics.**

- Termination: set semantics guarantees termination only for range-restricted
  rules over the finite set of facts that generate no new values.
- Rule shape: heads need a consistent arity and types.
- Stratification: no recursion through negation, so validation must reject
  negative dependency cycles. No aggregates inside recursive components,
  unless separately designed.

**Implementation.** SQLite's `WITH RECURSIVE` with `UNION` (not `UNION ALL`)
covers linear recursion. It limits multiple recursive references, so arbitrary
Datalog would need a fixpoint loop in Rust, or a restricted rule form. The
reference evaluator is a naive or semi-naive fixpoint.

**Historical views.** Every rule body reads the same view. The schema check
covers rule bodies too.

**Cost:** large. Parity testing for recursion needs its own generators.
**Value:** real for follow-up chains, but the chains are usually short, and
clients loop today.

### Nested pull

**Proposed form.** Pull patterns that follow references to a bounded depth,
returning nested maps.

**Placement.** This is a read API, not part of the query language.

**Contracts to keep and define.**

- It must keep `Db::pull`'s contracts: historical attribute validation,
  omitted absent fields, cardinality shape and value order.
- It must define the result shape at the depth limit, and how it handles a
  cycle along a path, for example by returning IDs for entities already
  visited.

**Cost:** medium. **Value:** convenience, with no request yet.

## Options

1. **Stage by value.** Stage 1: `not`, `avg` and HAVING. Stage 2: `or`.
   Stage 3: rules and recursion, only when a consumer needs chains. Nested pull
   is decided separately as a read API.
2. **Negation only now.** Only `not`, the one capability with a demonstrated
   need, and everything else waits for demand.
3. **All at once.** One larger design and implementation pass. This risks
   locking in semantics before use shows what is needed.
4. **None.** Clients keep working around absence and chains.

## Recommendation

1. **Now, as one item: `not` and `not-join`,** with the semantics above.
   - Absence is evaluated at the view's revision.
   - Unknown attributes are errors.
   - Variables are scoped lexically, and outer variables must be bound.
   - The SQL side uses a correlated `NOT EXISTS` with per-alias visibility.

   This is the only extension with demonstrated demand. It extends the
   existing SQL and reference parity tests, plus wire, CLI and pod coverage.
2. **Next, as a separately bounded item: `avg` and HAVING,** with exact
   rational rounding and an independent oracle.
3. **Later, only on demand:** `or` (with `or-join`) and recursion, each with
   the restrictions above.
4. **Separately:** nested pull, as its own read API item.

## Second opinion

Codex reviewed the first draft (2026-11-05, through kit). It agreed that
`UnknownAttribute` and a correlated `NOT EXISTS` are the right base for
historical absence, that HAVING belongs in aggregation before ordering, and
that nested pull is separate.

Its corrections, checked against the code and adopted:

- **The draft's absence example contradicted its own safety rule.** Absence
  needs `not-join` with a local variable, and validation needs lexical scopes.
- **`avg` must round once.** It divides as an exact rational, because
  converting the sum first rounds twice. It keeps the existing grouping,
  overflow and empty-input rules.
- **HAVING needs a defined scope.** It must stay out of the SQL tuple query,
  and needs an independent oracle, because SQL and the reference evaluator
  share the aggregation code.
- **Termination needs more than set semantics.** It also needs
  range-restricted rules that generate no new values, and no aggregates in
  recursive components. SQLite's `WITH RECURSIVE` cannot express arbitrary
  Datalog.
- **`or` branches need identical export types and deduplication before
  aggregation.**
- **Negation should come first, alone,** with `avg` and HAVING as a separate
  item. The recommendation now follows this.

## Decision (user, 2026-11-06)

Adopted the recommendation:

- **Now:** `not` and `not-join`, with the semantics above (plan item 12.2).
- **Next:** `avg` and HAVING, as their own item with an independent oracle
  (roadmap 5).
- **Only on demand:** `or` and recursion.
- **Separately:** nested pull, as a read API.
