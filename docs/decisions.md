# Implementation decisions

## Core identities and values

- Store/lineage token payloads contain 128 bits encoded as 32 lowercase hex
  digits. Tokens use `s1.` and `l1.` prefixes. Entity tokens use
  `e1.<store-hex>.<16-digit-allocation-hex>`, with zero allocation reserved.
  Callers parse/copy tokens; only the allocation catalog may allocate entities.
  Parsing a fabricated token never establishes membership.
- Compact snapshot tokens use
  `d1.<store-hex>.<lineage-hex>.<unsigned-decimal-revision>`. The planned public
  JSON representation remains the design's version/store/lineage/revision object.
  Snapshot operations will validate identity and availability, not merely syntax.
- Compact tokens require canonical spelling. Wire numeric strings may be
  normalized at their own boundary in item 4.1; do not confuse token spelling
  with idempotency's typed request comparison.
- Temporary labels preserve Unicode, reject empty strings/NUL, and remain
  distinct from allocated IDs. No alpha-renaming is performed.
- `FiniteF64` owns the finite/zero-normalization invariant. No public variant
  accepts a raw unvalidated float. `Value` implements typed equality/hash, and
  its explicit `compare` rejects cross-type ordering. It intentionally has no
  public total-order implementation over mixed types.
- Public errors have stable codes and typed context. Storage source errors
  and additional semantic context will be added with their implementation items.

The user requested unit, property, integration, and eventual `assert_cmd`
scenario testing. See `testing.md` for the verification policy.

## Batch normalization

- A private normalizer stages schema/entities, resolves facts, validates types,
  detects declarative conflicts, and computes old/new set differences. Its
  output is a candidate, not a commit. Evaluation adds preconditions and final
  uniqueness before reports are exposed to storage.
- Concrete IDs and concrete reference targets must be known at pre-head.
  Newly declared identities must use temporary labels, preventing guessed future
  allocated tokens from being accepted accidentally.
- Deterministic delta order is retract/assert, entity, attribute, typed value.
  No-op operations never appear. Internal tag ordering serves event encoding;
  it does not change public typed comparisons.
- The catalog's pre-integration lint expectation is removed. The private model
  has one temporary expectation until SQLite becomes its production caller.

## Schema and allocation

- Attribute names contain exactly one slash separating nonempty namespace and
  local name, with no whitespace/control characters. Unicode stays exact.
- Definitions have private fields and a validated constructor. No API renames
  or mutates committed type/cardinality/uniqueness. Schema projections are owned.
- The private catalog tracks creation revisions independently of facts. Staging
  clones the catalog; a failed schema/allocation batch cannot leak IDs or schema.
  A staged no-op still advances its proposed revision. SQLite will later persist
  the resulting events atomically.
- A temporary, documented dead-code expectation covers the catalog while it is
  integrated incrementally. Remove it when the evaluator becomes its production
  caller; the catalog is not a second public backend.

## Owned requests

- The builder owns its full request and validates declarations, temporary-label
  use, and scalar metadata. It preserves operation order and duplicates, and
  normalizes each expectation's exact typed set. Duplicate expectation entries
  remain submitted entries; state validation evaluates them all against pre-head.
- Schema, identity membership, cardinality conflicts, expected head, and
  expectations are resolved during evaluation/commit. They cannot run before
  retry lookup once idempotency exists. Building never borrows a SQL transaction.
- Repeated builder calls setting a metadata map key, expected head, or retry key
  replace that field. Wire duplicate JSON keys remain errors in item 4.1.
