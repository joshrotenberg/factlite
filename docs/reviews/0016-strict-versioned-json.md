# Item 4.1 self-review

Reviewed the complete recursive parser, typed interchange, dependency/lockfile,
and public integration-test diff.

- Duplicate keys fail at every object before a JSON map can overwrite them;
  escaped equivalent keys fail too. Unknown fields, missing required fields,
  wrong JSON types, malformed/extra tags, and trailing input fail.
- Values and tokens use validated constructors. No Deserialize derivation bypasses
  private invariants. i64/u64 boundaries, finite floats, zero normalization, byte
  padding/trailing bits, and recursion bounds have deterministic regressions.
- Decoding shares Tx structural validation, normalizes expected sets, and keeps
  submitted arrays in order. Explicit null is rejected; omission supplies defaults.
- Three 1,024-case properties cover exact value/request round trips and arbitrary
  parser input. Public tests exercise typed Rust/JSON equality and a durable commit.
- Formatting, Clippy with denied warnings, 85 tests (27 property suites), docs
  with denied warnings, and whitespace passed. Added serde/serde_json/base64 only.

No findings remain. This is self-review; remote CI has not run. Canonical retry
bytes, keyed commit, report/read output, and functional CLI follow in later items.
