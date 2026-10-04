# Item 4.2 self-review

Reviewed the complete frozen writer, public immutable evidence, and test diff.

- Fixed field order, explicit tags, and length/count prefixes make boundaries
  unambiguous. Request version is included, key excluded. Identity uses submitted
  typed inputs rather than head-dependent expansion or allocation.
- Signed integers and normalized raw float bits are encoded directly. Storage's
  sortable BLOB transformation and JSON decimal spelling do not enter this format.
- Golden empty/request-scalar bytes pin the representation. Deterministic tests
  distinguish schema/metadata/expectations/head/labels/operations, typed numeric
  values, duplicate changes, operation order, and renamed/reordered labels.
- Three 1,024-case properties cover expected-set permutations/duplicates, wire
  round trips, and distinct integer payloads. Alternate decimals, byte padding,
  zeros, defaults, keys, and map insertion order have equality regressions.
- Formatting, Clippy with denied warnings, 91 tests (30 property suites), docs
  with denied warnings, and whitespace passed.

No findings remain. Self-review only; no remote CI. Durable canonical evidence
and retry lookup must land together in the next increment before keyed commit.
