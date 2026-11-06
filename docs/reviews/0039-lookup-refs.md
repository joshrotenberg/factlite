# 11.2 Lookup refs for reads: self-review

Reviewed the complete diff. This is a self-review. No independent review or
remote CI ran.

## Change

`Db::lookup` validates the attribute at the view's schema (visible,
`unique_one`, matching type) and reads visible rows for the attribute and
value. `wire::encode_lookup` and a CLI `lookup` command expose it. The pod's
`EntityRef` is `{:id}` or `[attr value]`, resolved on the read's pinned view.
It is used by `pull`, `facts`, `history` and a new `lookup` var.

Self-review checked these points:

- At most one owner can match: unique ownership is validated at every
  revision, and a second owner is reported as storage corruption.
- Resolution and the read use the same `Db`, so they see the same revision.
- Unmatched lookups keep error behavior: the pod substitutes an identity
  bound to the all-zero store. Store identities are random version-4 UUIDs, so
  none is all zeros; the library defines foreign identities as matching
  nothing, and still validates attributes, filter types and history bounds.
- A lookup ref in a transaction fails `InvalidRequest` from the existing
  entity-term parser.

## Evidence

- Library: assignment, atomic transfer, retraction and historical views;
  unknown, non-unique, wrong-type and not-yet-defined attributes.
- Property: inside the 1,024 touched-commit traces, every lookup on the unique
  scalar and unique reference attributes at every revision equals a scan of
  replayed state.
- CLI: found, null, non-unique, wrong type and missing value.
- babashka (79 checks per transport): `lookup` at head and before assignment,
  `pull`, `facts` and `history` through lookup refs, unmatched lookups,
  validation still applied, and rejection in transactions.

Validation: `scripts/check.sh` passed on macOS 27.2, Rust 1.99.1, with 172
tests, the Rust workflow and warning-free rustdoc.

## Limitations

- Lookup refs are not accepted in transactions or as query constants. A query
  can join on the unique attribute instead.
