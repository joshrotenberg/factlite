# Review: item 1.5, constraints and reports

- **Branch:** `core/constraints-and-reports`
- **Reviewer:** Codex self-review.

Reviewed evaluator, context types, reports, scenario/property tests, and complete
diff. Conditions inspect the original catalog/facts; stage-local definitions and
IDs cannot satisfy them. Uniqueness checks complete candidate state, permitting
atomic swaps/transfers. Errors report exact typed sets and all duplicate owners.
No candidate state or report escapes a failed evaluation. Reports sort creation,
schema, and effective changes deterministically and copy submitted metadata.

Added success-biased expected-set properties so random tests exercise both pass
and failure paths. All six value types exercise expectation/transfer/uniqueness;
signed zero has specific exact-fact and expected-set regressions. A bounded
exhaustive truth table covers 768 batches in both orders and 96 expectations.
The table uses independent bit-set equations rather than reproducing the
normalizer's collection algorithm. The first lint run found a collapsible head
check; corrected it before validation.

`scripts/check.sh` passed: thirty-nine tests, sixteen 1,024-case properties,
public integration suites, scenario/table checks, formatting, Clippy, and docs.
No unresolved findings. Reopen/crash/contending writers/retry tests still require
the storage and CLI stages; the private model is not exposed as a second backend.
