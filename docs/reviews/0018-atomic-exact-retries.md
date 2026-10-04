# Item 4.3 self-review

Reviewed the complete commit/log/decoder, fault-suite, and public retry diff.

- Structural normalization and canonicalization precede BEGIN IMMEDIATE. Key lookup
  precedes every state-dependent check. Full canonical bytes decide replay/conflict;
  key/evidence/report/events/head share one atomic transaction and UNIQUE index.
- Retained decoder bounds every count, rejects truncation/noncanonical values,
  and re-encodes exact bytes. Replay verifies keyed requests produce their reports
  at the original pre-state. Unkeyed retained rows remain supported.
- Tests cover stale create/condition/head retries, original IDs/reports/metadata,
  changed metadata/schema/head/operation order/labels, new noops, failed-key reuse,
  eight-way same/different request contention, all types, cycles/empty entities,
  concrete reference expectations, and reopen. SQL uses bound keys, never literals.
- Existing eight-stage and COMMIT fault tests now use retry keys. Retrying after
  failures proves keys reusable; successful reports replay exactly after reopen.
- Three added 1,024-case properties cover every canonical prefix truncation,
  arbitrary retained bytes, and wire-equivalent retries with randomized Unicode
  keys after later commits/reopen. Corrupt, nonfinite, negative-zero, oversized,
  unknown-version, and canonical-but-report-mismatched evidence fails.
- Formatting, Clippy with denied warnings, 101 tests (33 properties), documentation
  with denied warnings, and whitespace passed.

No findings remain. Self-review only; no remote CI. Process/crash/CLI behavior is
subsequent work; SQL faults do not claim physical power-loss recovery.
