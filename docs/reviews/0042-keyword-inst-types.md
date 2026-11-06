# 11.5 Keyword and inst value types: self-review

Reviewed the complete diff. This is a self-review. No independent review or
remote CI ran.

## Change

`Value` and `ValueType` gain `Keyword` (validated newtype) and `Inst` (i64
milliseconds). The storage codec, canonical encoder and decoder, wire
decoder and encoder, and type-name mapping are extended; the wire type names
are centralized in one place. The storage DDL admits tags 0..=7. Open upgrades
older files through ordered steps, with 3 to 4 rebuilding `fact_events` and
`datoms`. The pod maps keywords and `#inst` in value positions.

Self-review checked these points:

- Appending the enum variants keeps every existing canonical order and tag;
  old reports, events and canonical requests decode to the same values.
- Validation sits in `Keyword::new`, used by every decoder (wire, canonical,
  storage), so invalid text cannot enter through any boundary.
- Blob ordering matches typed ordering: keywords compare like strings, and
  insts use the i64 sign-flip encoding. The codec property checks inst order.
- Query typing treats both as ordinary domains: range predicates need the same
  type, sums reject them, and min/max order them.
- The table rebuild copies every column explicitly and recreates both datom
  indexes. Open then requires the exact index set and full validation.
- The pod's only behavior change: a keyword in a value position was
  `InvalidRequest` and is now a keyword value. In a string attribute it fails
  `TypeMismatch`.

## Evidence

- Every typed property and fixture now covers eight types: replay parity,
  index parity, SQL query and aggregate parity, wire and canonical round trips,
  storage codecs and order, backups, and query validation domains.
- New unit tests for keyword validation, accessors and cross-type inequality.
- New format-3 upgrade test from real format-3 table definitions, with parity
  at every revision and keyword/inst commits afterwards.
- Pod: 95 babashka checks per transport, including keyword and inst round
  trips, a keyword constant in a query, an inst range with an `#inst` input,
  inst min/max, invalid keyword text and type mismatches.
- Manual: a backup copy of the clajent format-3 store upgraded in 247 ms. Its
  history at the last pre-upgrade revision is byte-identical to the previous
  CLI's output.

Validation: `scripts/check.sh` passed on macOS 27.2, Rust 1.99.1, with 181
tests (forty-nine 1,024-case properties), the Rust workflow and warning-free
rustdoc.

## Limitations

- Inst precision is milliseconds; no time zone or calendar is stored.
- Read-only opens of format 1 to 3 files fail until a writable open upgrades
  them.
- The format-3 to 4 rebuild copies both fact tables, so its time grows with
  history.
