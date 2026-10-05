# 7.2 Mixed stateful traces — self-review

Reviewed the complete test and documentation diff. No independent review or
remote CI ran. Production behavior is unchanged.

The oracle describes logical nodes and domain commands; it does not call the
private evaluator, apply reports to derive state, or trust returned IDs/deltas.
IDs use the established public identity encoding and allocation sequence.
Expected reports are calculated from independent before/after domain state,
including schema, temporary mappings, sorted effective events and metadata.

Sixteen command kinds cover conditional/fenced updates, many sets, unique
ownership and transfer, cycles and empty nodes, schema additions/redeclarations,
failed allocation, concrete future-ID rejection, exact/changed retries, no-ops,
and actual close/reopen. Failed keyed writes are followed by clean requests under
the same key, checking nonreservation. Current and historical reads check all
surfaces, including a join that projects away a many-valued variable to exercise
distinct. Reopen rechecks every historical basis and full original retry report.

The deterministic twenty-command scenario covers rare interleavings explicitly;
the shrinking property runs 1,024 sequences of up to twelve commands. This does
not enumerate all sequences, all type combinations or concurrent schedules.
The separate typed, concurrency, fault and process recovery suites remain needed.

Validation: the full local check script passed formatting, all-target Clippy
with warnings denied, 139 tests (forty-one 1,024-case properties), the asserted
Rust workflow and warning-free rustdoc. Cargo stayed offline; assert_cmd had
local socket permission. A final comment clarification was followed by format
and whitespace checks; executable behavior was unchanged.
