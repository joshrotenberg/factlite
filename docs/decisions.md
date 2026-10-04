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

## Constraints and reports

- Conditions resolve entities and attributes against pre-head. Exact typed sets
  normalize duplicates and reject incompatible types or one-valued sets with
  multiple distinct values. Proposed writes cannot satisfy their own conditions.
- Unique ownership is grouped in complete final state, allowing transfers/swaps.
  Multiple conflicts choose a deterministic attribute/value and list all owners.
- Reports include only effective additions/retractions and preserve scalar
  metadata, before/after tokens, allocations, and added definitions. Storage will
  persist them with the commit; the private evaluator has no persistence claims.
- Structured error context is boxed to keep error results compact while retaining
  exact expected/actual sets and ownership details.

## SQLite file baseline

- Use rusqlite 0.40 with bundled SQLite, and UUID v4 entropy for store/lineage IDs.
  Store clones share an `Arc<Mutex<Connection>>`; separate opens rely on SQLite's
  writer serialization. URI/in-memory modes are not part of the durable API.
- Configure FULL synchronization before initialization and after enabling WAL,
  foreign keys, and a two-second busy timeout. Bootstrap with BEGIN IMMEDIATE
  and commit all schema/identity state together. Change journal mode outside
  transactions, as SQLite requires.
- File markers are application ID `0x464c4954` and format/user version 1. Only
  empty files initialize; unrelated schemas, including view-only schemas, fail.
  Unsupported newer formats fail before changing owned tables.
- Revisions persist as eight-byte big-endian blobs so the full u64 domain orders
  correctly in SQLite. Tables are authoritative transactions, schema/entity/fact
  events, and store info. No current projection exists yet.
- Open checks required tables, singleton identity, and head/log maximum. Item 2.2
  adds full report/event/replay consistency; no complete corruption-validation
  claim is made before then. SQLite source errors remain attached in Rust.

## Authoritative encoding and replay

- Internal report encoding 1 begins with `FLR1`, uses bounded u64 byte/count
  prefixes, canonical tokens, explicit enum tags, and canonical map ordering.
  It is not the public JSON format or the future retry request encoding.
- Stored type tags 0–5 represent bool/i64/f64/string/bytes/ref. Integers flip
  the sign bit; floats flip positive sign bits and invert negative bits. Big-endian
  blobs then preserve same-type numeric order. Strings/bytes use exact bytes;
  references use canonical ID bytes. Decode rejects nonfinite or negative-zero
  stored floats and other noncanonical values.
- Retained reports and events must agree exactly, including ordinals. Replay
  validates contiguous revisions, identity bindings, allocation/schema events,
  effective deltas, final references/cardinality/uniqueness, and requested bounds.
  Reopen runs this check in its existing short SQLite transaction.
- No-op commits have reports/revisions and empty effective events. Persisted
  reports retain caller metadata and original temporary mappings for later retry.
- In the corruption test only, foreign keys are disabled explicitly to inject
  broken rows. Production connections always enable them and reopen verifies
  existing foreign-key consistency.

## Atomic commit outcomes

- `Store::commit` validates structure, begins IMMEDIATE, captures/replays pre-head,
  evaluates the request, and persists report/events/head together. It returns
  reports only after successful COMMIT. New no-change transactions advance head.
- Commit failures carry optional outcome evidence: RolledBack if no write was
  possible or explicit rollback succeeded; Unknown when a COMMIT error ended the
  transaction without conclusive outcome, or cleanup failed. Non-commit errors
  have no commit outcome. SQLite source errors remain attached.
- Retain the transaction guard through manual COMMIT execution to inspect its
  state; rusqlite's consuming commit method drops its guard and ignores rollback
  cleanup errors. There is no application cache published before commit.
- Keyed requests became supported together with exact lookup/persistence in 4.3.
  Retry lookup inside BEGIN IMMEDIATE precedes head/schema/entity/expectation
  validation. Matching canonical bytes return the original report; different
  bytes fail IdempotencyConflict. Fresh no-change requests still advance head.

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

## Pinned database ownership

- Db holds a Store clone and an immutable full snapshot token. Capturing head
  never leaves a SQL transaction open; each operation replays its pinned basis
  in one short deferred transaction and returns owned results.
- `db_at` verifies identity, lineage, retained revision bounds, and replayability.
  Dropping Store leaves views usable; closing all handles and reopening with
  the retained token recovers the same historical schema.

## Revision-bounded reads

- Fact filters combine exact entity/attribute/value restrictions. Named attributes
  must exist at the basis; supplied values must match their declared type.
  Value-only restrictions preserve type distinctions. Unknown/foreign entity
  restrictions match nothing; pull returns None for them.
- Flat pull projects exactly the supplied names, with duplicates collapsed and
  an empty list projecting no fields. Absent fields are omitted, known empty
  entities retain their ID, and references remain typed IDs. All names validate
  before checking entity availability. Many values use same-type ordering.
- Facts return deterministic entity/attribute/typed-value order. History returns
  effective events in (after, basis], retaining original ordinals after filtering.
  Transactions retain complete reports, metadata, and no-change revisions.
  Lower bounds above the basis fail InvalidRequest. The authoritative log is read
  within the same operation transaction as schema and replay validation.

## Public wire version 1

- Serde's recursive visitor parses objects before duplicate keys can be lost,
  including escaped equivalent keys. Serde JSON's default recursion bound stays
  enabled. Every typed object rejects unknown fields. Version is integer 1.
- Requests use schema/new_entities/changes/expect/metadata, optional expected_head
  and idempotency_key. Missing collections/maps default empty; missing optional
  fields mean absent. Explicit null never supplies a default. Schema definitions
  require all four fields. Structural validation is shared with the Rust builder.
- i64 strings permit signed decimal digits, including leading zeros and plus;
  unsigned revision strings permit decimal digits including leading zeros.
  Encoders emit canonical decimal spelling. Overflow and numeric JSON values fail.
  Floats use finite parsed f64 and round-trippable display; zero normalizes.
- Bytes accept the standard alphabet with or without padding; reject invalid
  trailing bits, whitespace, and URL-safe alphabet. Emit padded standard base64.
  Temporary references are confined to transaction change inputs; metadata
  excludes every reference. Values retain exact type distinctions.
- Interchange lives in `wire`, apart from SQLite/storage encoding. Typed tokens
  validate syntax on decode; db_at checks retained availability. Malformed snapshot
  fields fail InvalidSnapshot; malformed JSON itself fails InvalidRequest.

## Canonical request version 1

- Header FLC1, then four-byte big-endian request version, then fixed fields:
  schema, labels, changes, expectations, optional head, metadata. Collection counts
  and byte/text lengths are eight-byte unsigned big-endian. Defaults are encoded.
  The retry key is excluded. No current state or resolved temporary IDs enter.
- Schema entries encode exact name/type/cardinality/uniqueness; entity terms and
  value terms have separate concrete/temporary tags. Value tags are bool=0,
  i64=1, f64=2, string=3, bytes=4, ref=5. Integers are signed big-endian eight-byte
  values; floats are normalized raw IEEE binary64 big-endian bits. Strings/bytes
  remain exact. References and full head tokens use canonical identity text.
- Submitted arrays remain ordered, including duplicates in changes and expectation
  entries. Expected value sets are structurally sorted/deduplicated; metadata keys
  use exact lexical order. Renaming labels or reordering equivalent operations can
  conflict under the same key. CanonicalRequest exposes immutable version/bytes.
- Encoding is frozen apart from storage/report and public JSON codecs. Complete
  byte equality is the retry evidence; no digest-only decision. Key persistence
  and lookup landed together in item 4.3.

## Retained retry evidence

- Store canonical version/bytes and key on the transaction row in the same SQL
  transaction as reports, schema, allocations, fact events, and head. The UNIQUE
  key plus IMMEDIATE writer serialization makes concurrent retries allocate once.
  Failures reserve no key; keys have no expiry or pruning in v1.
- Retained decoding is length/count bounded, validates every typed input, and
  requires byte-for-byte canonical re-encoding. Unknown retained canonical versions
  fail UnsupportedFormat. Replay evaluates keyed requests against their original
  pre-state and verifies exact report agreement, alongside report/event checks.
- Matching retry lookup does not reevaluate current-state conditions. Reports
  preserve original snapshots, IDs, metadata, and effective delta after later
  commits and reopen. Same-key changed requests conflict before stale head checks.

## CLI version 1

- Thin std argument parsing delegates storage/semantics to the public library.
  Initialization is explicit; other commands require an existing path. Options
  reject unknown, duplicate, missing, or command-inapplicable flags before open.
  Native paths retain platform encoding; typed flags/JSON require UTF-8.
- Responses use public wire encoding and the supplied report/pull shapes. Other
  read envelopes contain version plus named arrays. History revisions/ordinals
  are decimal strings; transaction entries contain revision and full report.
  Unknown pull emits a null entity. No progress output appears on stdout.
- CLI owns exit taxonomy: invalid/type/entity/attribute/snapshot=2, batch/unique/
  condition/head/retry conflict=3, busy=4, storage/I/O/unsupported stored format=5.
  Errors carry stable code, typed context, and available commit outcome evidence.
  A stdout failure after commit is not rollback evidence; same-key retries recover
  original reports. Help/version are human-readable exceptions to JSON output.

## Query AST and validation

- Query names are exact nonempty strings without NUL, never SQL fragments. The
  AST has fixed attribute patterns, Var/Const/Param terms, typed input declarations,
  comparisons, selected names, selected-field ordering, and optional u64 limit.
  Negative Rust limits are unrepresentable; JSON will reject them at decoding.
- Validate against historical schema before matching, including limit zero.
  Entity positions are Ref; shared variables must have the same exact domain.
  Constants and parameters must match the position's domain. Query reference
  constants need no allocation check: absent facts simply fail to match.
- Equality/inequality use exact typed equality, including unequal mixed numeric
  types. Ranges require same-type operands. Predicates and projections require
  pattern-bound variables. Projection rejects duplicates; ordering is selected-only.
- Every declared input must be supplied with its exact type, even when unused;
  supplied undeclared inputs fail. Validation is pure against owned Schema/Params;
  Db validation obtains the schema through its common pinned read transaction.
- Empty conjunctions/projections are allowed; execution will use the relational
  identity row and distinct empty tuple where applicable. Limit occurs after
  distinct/order. No stable tie order or page boundaries are promised for a
  non-unique ordering. Reference evaluation precedes JSON and SQL execution.
