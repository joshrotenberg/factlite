# Review 0051: transaction lookup refs (12.3)

Branch `feat/tx-lookup-refs`, PR #16. Self-review plus independent Claude and
Codex subagent reviews, run with the kit subagent tooling. No human reviewer
and no remote CI ran.

## Plan second opinion (Codex)

Codex agreed with A1 and with resolving after the head fence and
expectations. Its points, checked against the code:

- **Error order across changes.** Resolving every lookup in a pass before
  normalization would let a later bad lookup beat an earlier unknown ID.
  Adopted: resolution moved into the normalization loop, so changes fail in
  submission order. A test pins both orders.
- **The reference evaluator needs its own resolver.** Adopted:
  `model_reference.rs` scans all facts instead of using the owners index.
- **Loader completeness.** The owner must be a named identity, its touched
  pairs loaded, and a resolved reference asserted on a unique attribute must
  load that value's current owners. Implemented by treating the owner exactly
  like an `Id` or `Ref`; a test covers a unique reference collision and a
  transfer through lookups.
- **Pod parsers are shared.** Adopted: lookup parsing is change-specific, so
  expectation values, metadata and params still reject collections.
- **Canonical decoding stays state-independent.** It does: tag 2 decodes
  without schema or ownership, and retries compare bytes before any state is
  read.

## Diff reviews

Neither review found a correctness bug. Findings and handling:

- **Orphaned comment in `model_reference.rs`** (Claude). Fixed.
- **Loader doc omitted lookups** (Claude). Fixed.
- **No deterministic test of a lookup on a value the same request retracts**
  (Claude). Added: it resolves to the old owner.
- **Pod bad-case assertions check only the code** (Claude). Not changed. The
  nested and temporary-ref cases are rejected by the wire decoder the pod
  delegates to, which `tx_lookups.rs` tests with the same inputs.
- **Canonical corruption test locates the tag by pattern** (Claude). Not
  changed. The pattern is the tag followed by the attribute's length prefix,
  which is unique in that request.
- **Generated traces carry no idempotency keys** (Codex). Not changed. Keyed
  replay of lookups on reopen is pinned by deterministic tests for entity and
  reference lookups, including after the value moved. Recorded in
  `status.md`.

## Self-review

- Before seeding, the shared generator produced 5 successful lookup steps in
  1,024 cases. With the seed state owning unique values it produces 468, so
  both parity properties now exercise resolved lookups, not only failures.
- An attribute defined by the same request is not visible to its lookups,
  because resolution uses the pre-transaction schema.
- Existing canonical bytes are unchanged; only requests that contain a lookup
  use tag 2.

## Validation

`REQUIRE_BB=1 ./scripts/check.sh` (fmt, clippy with `-D warnings`, tests,
docs, babashka pod tests) passed locally on macOS: 220 tests. GitHub
Actions did not run.
