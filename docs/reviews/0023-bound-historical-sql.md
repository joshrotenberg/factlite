# Item 5.4 self-review

Reviewed the full compiler/materialization/API, isolation, tests and docs diff.

- Historical schema and fact replay occur within the same read transaction as
  temporary materialization and SQL evaluation. No head projection is consulted.
  TEMP writes are private and cannot advance revisions or mutate durable tables.
  Cleanup drops before commit; injected failure rolls back and permits reuse.
- Query validation fixes the domain of every SQL expression. Attribute IDs,
  constants, parameters and limits bind as values. Generated numeric aliases,
  positional ordering and enumerated operators are the only generated SQL text.
  Quoted/SQL-looking names and NUL-bearing strings are safe and tested.
- Domain-separated equality handles identical byte payloads of different types.
  Same-type order-preserving encodings cover all six types and Ref/entity joins.
  DISTINCT, ordering and limit occur in the required order; zero validates first.
  SQLite's signed-cardinality ceiling uses unlimited -1 for larger u64 limits.
- Conservative SQL budgets retain reference fallback rather than reject valid
  larger ASTs; a 65-pattern regression exercises it. Randomized typed graph parity
  runs 1,024 cases through ordering/limits, later schema commits and old reopen.
  Existing independent integration oracles and all CLI fixtures now exercise SQL.
- Deterministic writer interleavings cover both head/replay and materialization/
  SQL reads. Both retain A while a separate WAL writer commits B.
- Formatting, denied-warning Clippy, 126 tests (39 properties), denied-warning
  rustdoc and whitespace passed after the final isolation addition.

No remaining findings. Self-review only; remote CI has not run. No durable cache
or projection introduced, and no performance improvement is asserted.
