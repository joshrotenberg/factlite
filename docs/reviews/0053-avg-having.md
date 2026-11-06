# Review 0053: avg and HAVING (12.5)

Branch `feat/avg-having`, PR #18. Self-review plus independent Codex and Claude
subagent reviews, run with the kit subagent tooling. No human reviewer and no
remote CI ran.

## Design second opinion (Codex)

The first plan review (run before implementation) was lost when the kit
server restarted, so Codex reviewed the implemented library commit instead.
It found no bug in the rounding or HAVING resolution. It checked the scaling
bounds (k from -74 to 116, shifted operands and doubled remainder fit u128),
the ties-to-even rule and the carry to 2^53, and matched a Python
transcription against exact `Fraction` conversion in 132,131 cases. Its
points, checked and adopted:

- **Rounding fixtures that small sums cannot reach.** Added `[2^53, 1, 0]`
  (exact mean 3002399751580331, which converting the sum first misses) and a
  rounding carry, next to the three double-rounding cases computed with
  Python's `Fraction`.
- **Limit zero skips aggregation.** Both evaluators return before `finish`,
  so an overflow cannot fail such a query. Documented on `Query::having`,
  `finish` and in `cli.md`, and pinned by a test.
- **Validation order was implicit.** Recorded in `decisions.md` and pinned by
  a test with competing HAVING, ordering and aggregate errors.
- **F64 mean rounds the sum at each step.** Accepted and documented as the
  ascending-order sum over the count.

## Diff reviews

Codex:

- **Pod HAVING could resolve an unselected aggregate to a selected variable**
  named like its column (`(avg ?cost)` against a selected `?avg-cost`). Fixed:
  a `(fn ?x)` term in `:having` must match an aggregate in `:select`, and a
  `?x` term a selected variable. Regression test added.
- **A babashka check had an ineffective predicate** (`(not= ?recipe "c")` with
  no recipe c). Fixed: the floor admits both groups and the predicate drops
  one.
- **F64 mean tests could not tell the documented sum from the exact mean.**
  Added `[-2^54, 1, 2^54]` in three input orders: the documented mean is 0,
  the exact mean 1/3.

Claude:

- **Status lacked 12.5.** Added.
- **`testing.md` said the HAVING threshold came from the data.** It is a random
  minimum group size; reworded.
- **The half-ulp property rarely reached small quotients and cannot check
  ties.** The numerator is now spread across bit lengths, and a new property
  builds exact ties and requires the even neighbor.
- **SQL parity shares `finish`, so it is no oracle for HAVING or `avg`.** Said
  so in the plan item and `testing.md`; the oracles are the grouped-aggregate
  property and the rounding unit properties.
- **`f64_sum` relied on callers to sort.** It now sorts its own input.
- **Formatting:** a missing paragraph break in `rust.md` and an overlong doc
  line in `query.rs`. Fixed.

## Self-review

- HAVING never reaches SQL: `tuple_query` clears it, and both evaluators call
  the same `finish`.
- The I64 sum cannot overflow i128 for any count that fits in memory, and the
  count is below 2^64, as `nearest_quotient` requires.
- Every type `avg` and HAVING produce flows through the existing wire value
  encoding; the CLI scenario checks `{"f64":"1.6666666666666667"}` exactly.

## Validation

`REQUIRE_BB=1 ./scripts/check.sh` passes locally: 228 tests, including
fifty-four 1,024-case properties and 113 babashka checks. GitHub Actions is
unavailable for this account this month.

## Limitations

- HAVING runs in Rust over aggregated rows; there is no SQL form.
- The F64 mean is not the correctly rounded mean of its inputs. It is
  deterministic and documented.
- The SQL parity property does not independently check HAVING or `avg`.
