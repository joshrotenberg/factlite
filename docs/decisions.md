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
  (since 9.3, only initialization and upgrade take it; see below)
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
  never leaves a SQL transaction open; each operation loads its pinned basis
  in one short deferred transaction and returns owned results. Until 9.1 this
  load was a full replay; it now reads the verified datom index.
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

## Reference query evaluation

- Validate the full query first, then join each conjunctive pattern on exact typed
  bindings, filter predicates, project, remove duplicate tuples, order selected
  fields, and limit. Zero limit returns empty only after validation. Missing facts
  fail matching without inventing null values.
- The private evaluator consumes a replayed fact set, not a public second backend.
  Entity positions bind Value::Ref, including same-pattern self-edge unification.
  Comparisons and ordering reuse the value domain's numerical/byte/identity rules.
- Empty conjunction yields one empty binding. Projection onto no columns yields
  the distinct empty tuple if any binding survives, otherwise zero rows. Every
  returned tuple/result is owned. Unordered or non-total tie order is unspecified.
- Db::query uses the common pinned read transaction. The forced WAL interleaving
  suite now covers query alongside the five original public read surfaces.

## Query interchange

- Version-one envelopes require query.match and query.select. Omitted inputs,
  where/order, and params default empty; omitted limit is unbounded. Null, unknown
  fields, duplicate keys, ambiguous terms, and temporary references fail strictly.
- Terms use a single var/param tag or the existing lossless concrete-value tags.
  Predicates use left/op/right with eq/ne/lt/le/gt/ge. Limit uses a nonnegative JSON
  u64 integer. The design's exact joined example is checked in and executable.
- JSON decoding checks structure; Db's historical type/binding validation is the
  common semantic boundary for Rust and CLI. Results name columns and retain
  typed tuple values, including references in entity positions. CLI query reuses
  pinned view flags and rejects two stdin-consuming inputs before reading.

## Historical SQL bridge

- Db::query reconstructs its exact historical schema/facts inside the common
  short read transaction, then materializes a connection-local TEMP relation.
  Query never commits main-database writes or reads a newer current projection.
  Drop the TEMP relation before success; transaction rollback removes it on error.
- Resolve fixed qualified attributes to numeric IDs from the pinned schema. SQL
  text uses only generated numeric aliases/positions and enum-selected operators.
  Attribute IDs, concrete values, parameters, and limits are bound. Variable,
  column, attribute, and parameter names never become SQL fragments.
- Fixed attribute domains type each value column; per-type order-preserving BLOBs
  retain exact comparisons. Entity columns use identical Ref encodings, allowing
  entity/reference joins. Cross-type Eq/Ne compile to false/true rather than
  comparing coincidentally identical bytes. No SQLite numeric/text coercion.
- SQL uses DISTINCT projection, selected positional ordering, then bound limit.
  SQLite cardinality is signed-64 bounded: larger u64 limits bind unlimited -1.
  Preserve valid AST semantics with reference fallback above conservative budgets
  of 32 patterns, 128 predicates, 256 selected columns or 256 order entries.
- Deterministic fixtures and 1,024 randomized typed graph cases compare SQL to
  the reference, with later writes and reopened old views. Forced writers now
  occur both before replay and between materialization and query reads. Failures
  during TEMP writes roll back and leave the connection reusable. This is a
  historical correctness bridge; performance decisions follow measurements.

## Process recovery evidence

- Test-only child re-entry pauses at the pre-COMMIT hook or after successful
  COMMIT, before a report can be returned. Cache pressure and a large canonical
  request/report ensure WAL frames exist at both checkpoints. Parent handshakes,
  abrupt kills and reopened full-state/retry assertions replace scheduling sleeps.
- Pre-COMMIT termination leaves the old report, schema, entities, facts, metadata
  and key state. Post-COMMIT termination recovers the complete original report
  through the same key despite its stale head fence. All subsequent retries match.
- Separate assert_cmd processes start behind barriers. Eight same-key creates
  return identical reports and allocate once; eight conditioned updates have
  exactly one winner and seven confirmed rollback errors. Bounded busy, malformed
  storage and unsupported format checks remain in the existing gate.
- These are process/WAL recovery tests, not physical disk or power-failure tests.

## Safe backup and read-only access

- Enable rusqlite's backup feature; promote the already locked tempfile dependency
  to production for exclusive same-directory staging and no-replace publication.
  Fix one source read snapshot, validate it, run SQLite backup, validate destination
  full history/token, enforce DELETE journal mode, close/sync, publish without
  replacing existing paths, and sync its directory on Unix. No live-file copying.
- Return the snapshot actually copied. An independent WAL writer can advance
  concurrently without leaking newer revisions into the copy. Preexisting main
  paths or sidecars fail; a raced-in destination survives unchanged. Ordinary
  errors clean staging. Portable publication follows tempfile's documented
  guarantees; directory sync errors may happen after publication.
- Db::open_read_only and open_read_only_at return pinned owned views backed by
  SQLITE_OPEN_READ_ONLY, without creating/init/WAL-setting main database writes.
  TEMP query DML remains allowed. CLI read commands adopt these constructors;
  init/transact retain writer access. I/O errors keep their source and context.
- Backup IDs/lineage/tokens/reports/retry evidence are preserved exactly. Resuming
  the lineage requires identical complete retained history and retiring the old
  writer. Older incomplete restores and divergent writable clones are unsupported
  caller-enforced restrictions, with no automatic detection or relineaging API.

## Measured optimization decision

- Release workload runner checks independently expected typed joins and measures
  head capture, validated read-only open, revision-1/head queries, fact reads and
  new no-op commits. Record four task/history configurations with 15 samples each,
  separately on tmpfs and workspace overlay; full raw JSON is checked in.
- Replay/validation scales with retained revision count and state size. At 1,000
  tasks/50 revisions, workspace head-query median is 181.939ms and validated open
  178.738ms, versus 23.646ms for revision-1 query. These are observed local-container
  costs, not an agreed latency goal or durability-device characterization.
- Keep existing indexes and replay for now. No workload/SLA exists to select a
  checkpoint cadence/cache budget, and the measurements do not justify extra SQL
  indexes. Future performance work should target repeated replay with explicit
  immutable-state/checkpoint design, reference parity and forced failure/isolation
  checks. No speculative derived persistent state or format change was introduced.

## Verified evaluated-state reuse (7.3)

- Stage profiling showed duplicate report application costs roughly half of
  replay wall time in the larger keyed fixtures. Keyed replay already decodes
  the retained canonical request, evaluates it at the preceding state, and
  compares its complete generated report with the persisted report.
- Reuse that evaluated state only after exact report equality and all event-row
  checks. Evaluation establishes a valid final state and canonical declarations,
  IDs, events and metadata. Inductively, starting from the empty valid catalog,
  reapplying the identical report adds no validation evidence. Unkeyed reports
  retain report application and full state validation.
- Storage format, short SQL snapshots and failure publication behavior remain
  unchanged. No mutable cache, checkpoint or projection exists. Retain the old
  double-reconstruction path in test builds; compare typed mixed prefixes,
  historical schema/entities/facts and subsequent allocations. Corruption tests
  modify reports and matching event rows together to verify fail-closed behavior.
- Fresh before/after workload medians and stage results are in `measurements.md`.
  This removes demonstrated redundant work without choosing a cache budget or
  claiming a production latency target. Full-history replay still scales with
  retained revisions and live state size.

## Temporal datom index (9.1)

- Format 2 adds one derived table, `datoms`: a validity interval per effective
  assertion with columns entity, attribute, type tag, value, `asserted` and
  nullable `retracted` revisions. A fact is visible at R when `asserted <= R`
  and `retracted` is null or greater than R. A retract-then-reassert cycle keeps
  separate intervals. The primary key is (entity, attribute, tag, value,
  asserted); a partial unique index allows at most one open interval per fact.
- Columns reuse the fact-event encodings: canonical entity and attribute text
  with foreign keys to their events, explicit type tags, and the per-type
  order-preserving value blobs. Revision columns are eight-byte big-endian blobs
  with checks. Numeric entity/attribute keys are deferred until measured.
- `persist_report` applies the effective delta inside the commit transaction:
  assertions insert an open interval and retractions close exactly one open
  interval. Any failure rolls back with the rest of the commit. The table is a
  deterministic function of retained fact events; it is never an authority.
- Every open, writable or read-only, still validates the complete retained log by
  replay, then requires stored intervals to equal those derived from fact events,
  then requires the indexed state at head to equal the replayed state. Mismatch
  fails closed with `Storage`. Open therefore stays proportional to history.
- Reads and commit evaluation load the state at R from schema and entity events
  bounded by R plus visible intervals, inside the operation's single SQLite
  transaction, instead of replaying transactions. The catalog's next allocation
  follows the maximum retained allocation, since allocation is contiguous from
  one. Basis identity and bound checks are unchanged.
- Integrity is established at open. Reads after open no longer revalidate the
  whole log, so rows altered by external SQL writes while a store is open are
  not detected until the next open. External writes to Factlite-owned tables
  remain unsupported, as the design states.
- A writable open of a format-1 file upgrades it in its existing IMMEDIATE open
  transaction: create the table, derive intervals, set format 2, then run the
  full format-2 validation. Any failure rolls back and leaves the format-1 file
  unchanged. Read-only opens of format-1 files fail `UnsupportedFormat`, since
  they cannot write the index. Formats above 2 fail closed as before.
- Queries still materialize a TEMP relation from the loaded state. Answering
  facts, pull and query with revision-bounded SQL directly over intervals is
  item 9.2.

## Babashka pod (10.1)

- Authorized by the user on 2026-11-05 for the clajent agent kit. The crate is
  `factlite-pod` with binary `pod-factlite` and namespace
  `pod.joshrotenberg.factlite`. It is built on the published `babashka-pod`
  0.1.1 crate rather than a path dependency on a sibling checkout, so the
  lockfile pins it.
- Use transit+json and `function_transit` handlers. Transit keeps keywords,
  symbols, byte arrays and the long/double distinction, which the mapping
  depends on. The pod runtime turns handler errors and panics into error
  replies and keeps serving.
- Inputs are translated into version-1 wire JSON and decoded by
  `wire::decode_tx`, `decode_query`, `decode_snapshot` and `decode_value`. The
  pod adds no second validator. A pod request and the equivalent CLI JSON decode
  to equal `Tx` values with identical canonical bytes, so they share retry
  identity. Outputs are built from library result types directly.
- Value mapping is one Clojure type per Factlite type: boolean, long, finite
  double, string, byte array, and `{:id ...}` for references (`{:temp ...}` in
  changes). Everything else in a value position is `InvalidRequest`. Keywords
  are not accepted as string values, so no value changes type on a round trip.
- The Clojure shapes are idiomatic, not a key-for-key copy of the wire: kebab-case
  keys, `[op entity attr value]` changes, `[entity attr values]` expectations,
  `?var`/`$input` query symbols, `(op left right)` predicates, and flat pulled
  entities (`{:id ...}` plus attributes, which always have a namespace). Envelope
  `version` fields are dropped from outputs; snapshots keep theirs.
- Reads accept `:at` as a full snapshot map or a revision number. A number is
  resolved against the connection's store and lineage, then validated by
  `Store::db_at` like any token.
- Errors carry `{:code :message :context :commit-outcome}` taken from
  `wire::encode_error`, with context values converted by their fixed keys.
  Pod argument errors use the same shape with code `InvalidRequest`.
- Not exposed: read-only opens, backups and raw wire JSON. Registry publication
  and release packaging are not part of this item.

## Open without the write lock (9.3)

- Reported by the clajent session from pod trials: `Store::open` validated the
  whole log inside `BEGIN IMMEDIATE`, so every open held the write lock for its
  full validation. At 2,803 revisions that was 7.5 s, during which other
  processes' commits and opens failed `Busy` after the two-second timeout.
- Open now classifies the file and validates an existing current-format store in
  a deferred read transaction. Under WAL that snapshot neither blocks writers nor
  waits for them. Validation covers one consistent snapshot; commits made after
  it are validated by the next open. Only initializing an empty file and the
  format-1 upgrade write, so only they take IMMEDIATE. They classify the file
  again once the lock is held, so concurrent first opens create one identity.
- Open time is unchanged: full replay validation is still proportional to
  revisions times live state. That is a separate decision.

## Incremental replay validation (9.4)

- Chosen by the user on 2026-11-05 over checkpoints and an explicit verify
  step, after the clajent session measured 7.5 s opens at 2,803 revisions.
  Profiling showed every transition cloning, re-validating and diffing the
  complete fact set and cloning the entity catalog, so replay cost was
  revisions times live state.
- Transitions now run in place on an owned model. Commit and replay consume
  their model; borrowing `evaluate`/`apply_report` exist for tests. Catalog
  staging mutates in place, and concrete identities are checked against the
  pre-head revision, as before.
- The model keeps two indexes, built on the first transition and maintained by
  every fact change: values per entity/attribute, and owners per unique
  attribute/value. Read paths never build them.
- Checks are restricted by an induction argument. The empty model is valid. A
  transition never removes entities or definitions, so untouched facts stay
  known and typed. Only asserted facts can break cardinality one, typing or
  unique ownership. Only touched facts can differ between the states before and
  after, so the effective delta is computed from them. Each transition
  therefore checks its asserted facts and touched facts only, and validity holds
  at every revision.
- Error selection is preserved. A unique violation reports the least
  attribute, then least value, with all owners, as the full scan in
  attribute/value order did. Replay maps every validation failure to `Storage`
  either way.
- `model_reference.rs` keeps the former full-state transitions verbatim in test
  builds. `log::replay_reference` uses them, and evaluates keyed records then
  reapplies their reports, so replay parity compares against the most
  conservative path.
- Not changed: open still replays the whole log, so open is linear in retained
  revisions. Commits and reads still load the complete state at head, so they
  are linear in live state (item 9.2).

## Direct indexed reads and format 3 (9.2)

- `schema`, `facts`, `pull`, `history`, `transactions`, `validate_query` and
  `query` no longer load the state at R. Each read checks its basis inside its
  SQL snapshot, then reads schema events bounded by R, entity events for
  membership, and `datoms` rows visible at R
  (`asserted <= R AND (retracted IS NULL OR retracted > R)`).
- Queries compile to SQL over `main.datoms`, one alias per pattern, each bound
  to the view's revision. No TEMP relation is created. Every variable has a
  BLOB expression for value comparison, selection and decoding. A variable
  first bound at an entity position also keeps the TEXT entity column, so later
  entity positions join on `entity = <text>` and can use the primary key.
  Entity constants bind canonical identity text. Attribute names bind as text.
  Large ASTs beyond the SQL budget still use the reference evaluator, now
  loading the state at R only for that case.
- Format 3 adds `datoms_avet (attribute, value, entity, asserted, retracted)`.
  An attribute fixes its value type, so the type tag is not part of it. Open
  requires the exact datom index set. Writable opens upgrade format 2 by adding
  the index, and format 1 by creating the table and both indexes, in the
  upgrade transaction under the write lock. Read-only opens of formats 1 and 2
  fail `UnsupportedFormat`.
- Results are unchanged in value and order: facts and pulled values are still
  sorted in Rust by canonical order, and queries keep the former SQL for
  distinct, order and limit.

## Commits load only touched state (9.5)

- `index::model_for_request` builds the commit model from: all definitions at
  head; the identities the request names (change entities, reference values,
  expectation entities and values) with their creation revisions; the next
  allocation; every visible value of each entity/attribute pair the request
  changes or expects; and every visible owner of each unique value it asserts.
- Evaluation reads nothing else. It checks membership only for named
  identities, reads values only for touched pairs and cardinality-one
  replacements (both loaded), tests fact membership only for touched facts,
  and counts owners only for asserted unique values. So the result equals
  full-state evaluation; a differential property checks this, including errors.
- The next allocation is read as the greatest entity text. One store's
  identities share a prefix and a fixed-width lowercase hex allocation, so text
  order is allocation order. A test compares it with the full catalog's value.
- Open, reads and replay are unchanged. Open still replays and validates the
  whole log, so its cost stays linear in revisions plus live state.

## Open options and read-only pod connections (11.1)

- `StoreOptions` carries the busy timeout for `Store::open_with` and
  `Db::open_read_only_with`; `open` and `open_read_only` keep two seconds. The
  timeout must be whole milliseconds no greater than `i32::MAX` (SQLite's
  unit and limit); zero fails at once.
- `Db::at_head` and `Db::at` return new views over the same connection. A view
  still never refreshes itself; these are explicit and do not reopen or
  revalidate the file, which matters now that open replays the whole log.
- The pod's connection registry holds a writer `Store` or a read-only `Db`.
  Read-only connections read at the latest head on each call, as writer
  connections do, and `transact` on them fails `InvalidRequest` before any
  storage access.

## Lookup refs for reads (11.2)

- `Db::lookup(attribute, value)` returns the entity whose `unique_one`
  attribute has exactly that typed value at the view's revision, or None. A
  missing attribute fails `UnknownAttribute`, a non-unique attribute
  `InvalidRequest` (attribute context), and a wrong value type `TypeMismatch`.
  It reads one attribute/value index range. It never creates, merges or upserts.
- The CLI adds `lookup --attr ATTR --value TAGGED_JSON [VIEW]`, emitting
  `{"version":1,"entity":{"id":...}}` or `{"version":1,"entity":null}`.
- The pod accepts `[attr value]` wherever a read takes an entity (`pull`, and
  `:entity` in `facts`/`history` filters) and adds a `lookup` var. A lookup
  resolves on the same pinned view as the read. When it matches nothing, pull
  returns nil and filters match nothing, while attributes, bounds and options
  are still validated: the pod substitutes an identity bound to an all-zero
  store, which no store has.
- Transactions do not accept lookup refs. Resolving them at pre-head would add
  request terms to the canonical encoding and retry identity; that needs its
  own decision.

## Indexed filtered history (11.3)

- Reported by the clajent session: entity-filtered history took 37 ms when
  other reads took about 1 ms, because history decoded every report in range.
- When a history filter names an entity or attribute, events come from datom
  intervals instead: an assertion at `asserted` and a retraction at
  `retracted`, each within `(after, R]`, joined to that revision's fact events
  for the original ordinal, ordered by revision and ordinal. No format change:
  the entity primary key, the attribute/value index and the fact-event
  primary key serve it.
- Unfiltered and value-only history still read every report in range, which
  is the reference path in tests.

## Aggregates (11.4)

- Chosen by the user on 2026-11-05. The original design listed aggregates as
  outside v1; the design document is unchanged and this records the extension.
- `Query` gains `with: Vec<QueryName>` and `aggregates: Vec<Aggregate>`
  (`function`, `var`, `name`). Without aggregates, behavior is unchanged, and
  `with` requires at least one aggregate.
- Evaluation extends match, filter, project, distinct, order, limit. The query
  projects the selected variables, the `with` variables and each aggregate
  variable; removes duplicate tuples; groups by the selected variables;
  aggregates; then orders and limits over selected and aggregate columns.
  Aggregating distinct tuples follows the existing set semantics and Datomic's
  `:with`. Equal values from different matches collapse unless a
  distinguishing variable is in `with`. Documentation states this.
- Functions: `count` (tuples in the group), `count_distinct` (distinct values
  of the variable), `sum` (I64 or F64 only, otherwise `TypeMismatch`), `min`
  and `max` (any type, by same-type order). Counts are I64.
- Arithmetic is exact or an error, never wrapped. An I64 sum overflow fails
  `InvalidRequest`. F64 values are summed in ascending order, so the result
  is deterministic for a given set of tuples, and a non-finite running total
  fails `InvalidRequest`.
- Empty input: Factlite has no null for a missing sum or min, so no matching
  tuples give no rows, with or without grouping. This differs from SQL's
  single row for an ungrouped aggregate.
- SQL produces the distinct tuples, using the existing compiler, and
  `query_aggregate::finish` groups and computes for both the SQL path and the
  reference evaluator. SQLite's own SUM is not used: its integer overflow
  behavior and float summation order are not ours.
- Wire: `query.aggregates` entries `{"fn","var","name"}` with fn `count`,
  `count_distinct`, `sum`, `min` or `max`, and `query.with` names, both
  optional and emitted only when non-empty. The pod writes `(fn ?var)` in
  `:select` and `:order`, names the column `fn-var`, accepts `:with`, and
  returns columns in `:select` order.

## Keyword and inst value types (11.5)

- Chosen by the user on 2026-11-05. The design allowed new types "only with
  explicit comparison and encoding contracts"; these are the contracts.
- `ValueType::Keyword` / `Value::Keyword(Keyword)`: text `name` or
  `namespace/name`, without a colon. Both parts are nonempty, with no `/`,
  whitespace or control characters, so every keyword reads back as the same
  Clojure keyword. Equality is exact text; ordering is bytewise, like strings.
  A keyword never equals a string.
- `ValueType::Inst` / `Value::Inst(i64)`: a UTC instant in signed milliseconds
  since the Unix epoch, matching Clojure `#inst` and transit precision, over
  the full i64 range. Ordering is numeric. An inst never equals an i64.
- Both variants are appended to the enums, so existing values keep their
  canonical cross-type order.
- Storage tags 6 (keyword, UTF-8 text) and 7 (inst, sign-flipped big-endian
  eight bytes, ordering like i64). Decoding rejects invalid keyword text and
  noncanonical bytes, as for other types.
- Canonical request encoding adds value and type tags 6 and 7 (text; eight
  big-endian bytes). Existing canonical bytes are unchanged, so retained retry
  identities are preserved without a canonicalization version change.
- Wire: `{"keyword":"..."}` and `{"inst":"<signed decimal>"}` (decimal parsed
  like i64), type names `keyword` and `inst` in schema and query inputs.
- Format 4. Format 3's `fact_events` and `datoms` CHECK constraints admit tags
  0..=5 and SQLite cannot alter a CHECK, so the upgrade rebuilds both tables:
  create, copy, drop, rename, then recreate the datom indexes. Neither table is
  referenced by a foreign key. Upgrades are now ordered steps (1 to 2, 2 to 3,
  3 to 4), each recorded in `store_info.format` and `user_version`, all inside
  the open's write transaction and followed by full validation.
- Pod: Clojure keywords in value positions are keyword values, and `#inst`
  values are insts. Keywords in pod-defined positions (attribute names, labels,
  enum options) keep their meaning there.

## License (2026-11-05)

- The user chose MIT OR Apache-2.0, the usual Rust dual license and the same as
  `babashka-pod`. `LICENSE-MIT` and `LICENSE-APACHE` are at the root, every
  crate sets `license.workspace = true`, and the README states it. Crates stay
  `publish = false`; publishing to crates.io is a separate decision.
- The Elixir NIF (roadmap item 7) is deferred by the user.
- The pod is not published to the babashka pod registry (user decision,
  2026-11-05). It stays in this repository and is built from source.

## JSON-RPC interface: designed, deferred (2026-11-05)

- The user chose to design a JSON-RPC 2.0 process-boundary interface now and to
  build it later, as its own crate in this workspace (`crates/factlite-rpc`).
  The design is `design/json-rpc.md`, and roadmap item 10 tracks it. The library
  remains the core. No server code exists, and `AGENTS.md` still excludes
  building one until authorized.

## Roadmap decisions (2026-11-06)

The user decided four briefs in `decision-briefs/`; plan step 12 implements
them.

- **Open-time integrity:** full validation by default; a structural open for
  read-only connections only; explicit verify.
- **Transaction lookup refs:** option A1. Expectation lookups come later, with
  canonical request version 2.
- **Query language:** negation now; then `avg` and HAVING; `or` and recursion
  on demand.
- **History:** immutable. Documentation of permanence and application-level
  encryption; a lineage rewrite only if needed; no in-place excision.

## avg and HAVING (12.5)

- `AggregateFn::Avg` takes I64 or F64 variables (anything else is
  `TypeMismatch`, as for `sum`) and returns F64. It follows the existing
  distinct-tuple, grouping, `with` and empty-input rules.
- An I64 mean sums exactly in i128, which cannot overflow for any count that
  fits in memory, and divides the rational sum/count with one rounding to the
  nearest f64, ties to even. Converting the sum to f64 first would round twice.
- An F64 mean divides the ascending-order f64 sum, as `sum` computes it, by
  the count. The sum rounds at each step, so this is not the correctly rounded
  mean of the inputs; it is deterministic. A non-finite running sum is an
  error, so `[MAX, MAX]` fails even though its mean is representable.
- `Query::having` holds predicates in the `where` shape. A variable in HAVING
  names a result column: a selected variable or an aggregate name. Pattern
  and `with` variables that are not result columns are `InvalidRequest`. An
  aggregate named like a non-selected pattern variable is that aggregate in
  HAVING. Constants and declared parameters work as in `where`; range
  comparisons need one type. HAVING needs at least one aggregate.
- Evaluation: every group is aggregated, then HAVING filters, then ordering
  and the limit. An aggregate error in a group HAVING would drop still fails
  the query. A limit of zero returns before aggregating, so it cannot fail.
  HAVING never reaches the SQL tuple query.
- Validation order: inputs, patterns, predicates, negations, selection, `with`,
  aggregates (binding, type, then name), HAVING (left term, right term, then
  comparison types), ordering.
- Wire: `"fn":"avg"` and an optional `"having"` array, encoded only when
  nonempty. Pod: `(avg ?x)`, and `:having` where `(fn ?x)` names that
  aggregate's column.

## Transaction lookup refs (12.3)

- Option A1 of the brief: `EntityTerm::Lookup(attr, value)` and
  `ValueTerm::LookupRef(attr, value)` in changes only. Expectations keep
  `EntityId`; expectation lookups wait for canonical request version 2.
- Resolution happens inside `normalize_in_place`, change by change in
  submission order, against the pre-transaction schema and facts. No fact
  changes until every change is resolved, so the owners index is still the
  pre-state's. Resolving inside the loop, not in a pass before it, keeps
  errors in submission order: an earlier unknown ID beats a later unmatched
  lookup, and the reverse. The head fence and expectations come first, as
  before.
- Checks per lookup, in order: attribute visible before the transaction
  (`UnknownAttribute`, including an attribute defined by the same request),
  `unique_one` (`InvalidRequest`), value type (`TypeMismatch`), owner
  (`UnknownEntity` with the new `ErrorContext::Lookup`). No upsert.
- After resolution the IDs are ordinary IDs, so an ID and a lookup naming one
  entity are one fact before the conflict and cardinality checks.
- Canonical tag 2 in the entity-term and value-term positions: attribute text,
  then the value. Canonical version stays 1 and existing bytes are unchanged.
  Decoding is state-independent, so a retry compares the lookup, not its
  resolution, and an exact retry returns the original report after the value
  moves. Replay on open re-evaluates retained keyed lookups against each
  historical pre-state.
- The commit loader resolves each lookup at head with the same rules, loads
  the owning fact, and treats the owner like an `Id` or `Ref`. Lookups that
  evaluation will reject load nothing, so evaluation reports them in order.
- The reference model resolves lookups by scanning all facts, independent of
  the production owners index. The shared transaction generator seeds owned
  unique values so generated lookups usually resolve.
- Wire: `{"lookup":{"attr","value"}}` as a change entity and inside `ref` as a
  change value. Pod: `[attr value]` (vector or list, as for read lookups) in
  the same two positions. Errors encode the context as
  `{"lookup":{"attr","value"}}`.

## Query negation (12.2)

- `Query` gains `negations: Vec<Negation>`. A `Negation` has `join`
  (`None` for `not`, `Some(vars)` for `not-join`), `patterns` and
  `predicates`. Groups are one level deep.
- A binding of the positive patterns that passes the outer predicates
  survives only if no extension of its join variables (all outer variables for
  `not`) satisfies every group pattern and predicate at the view's revision.
  Negation runs before projection, distinct tuples and aggregation.
- Scopes. Outer variables come from positive patterns only.
  - `not`: every group variable must be an outer variable.
  - `not-join`: join variables are distinct outer variables. Other group
    variables are local, must be bound by a group pattern (not only a
    predicate, and in any order), and must not share a name with an outer
    variable. Rejecting shadowing tightens the brief's wording: a reused name
    is more likely a mistake than an intended fresh variable.
  - Sibling groups have separate scopes, so a local name may have different
    types in each.
  - An empty join list and unused join variables are valid. An empty group is
    invalid, though well defined, because it would remove every row.
  - Group attributes unknown at the view are `UnknownAttribute`. All of this
    runs before data is read, including with limit 0.
- SQL: each group is a correlated `NOT EXISTS (SELECT 1 [FROM ...] WHERE ...)`.
  Group aliases carry the attribute, type-tag and visibility conditions of
  outer patterns. Join variables reuse the outer binding forms, so entity
  positions compare TEXT and value positions BLOB as before. Typed comparison
  folding uses the group's own types. Group patterns and predicates count
  toward the SQL budget for the reference fallback.
- Wire: optional `query.not`, an array of `{"join"?, "match"?, "where"?}`.
  Absent `join` is `not`; `"join":[]` is a `not-join` with no join variables.
  Encoding omits `not` when there are no groups, so existing envelopes encode
  unchanged.
- Pod: `(not clause ...)` and `(not-join [?var ...] clause ...)` in `:where`,
  with vector clauses as patterns and list clauses as comparisons.

## Read-only structural open and verify (12.1)

- `Validation::Full` stays the default. `Validation::Structural` checks format
  markers, the table and index sets, and that the head row matches the latest
  transaction. `Store::open_with` refuses it before opening or creating the
  file, because commits trust the stored index.
- `Store::verify` and `Db::verify` run full validation in one read transaction
  and return the validated head. Errors keep their codes. Verify holds the
  handle's connection while it runs.
- Reads no longer assume full validation ran:
  - decoded facts, from intervals or reports, must match the visible schema's
    attributes and types;
  - compiled queries match on the stored type tag, so a mistyped row never
    matches and is never reinterpreted;
  - a snapshot must name a retained revision;
  - report reads require contiguous history;
  - `pull` fails on several visible values for a one-valued attribute.

  On a corrupted file, a structural view returns `Storage` errors or answers
  from the stored index, never a panic. Wrong answers are possible until
  verify runs.
- CLI: `--validation full|structural` on read commands, and `verify`, which
  opens structurally and then validates fully, so it validates once. Pod:
  `:validation` on `open`, and `verify`.

## Work queue pattern and agent conventions (13.1, 13.2)

Chosen by the user on 2026-11-23. Guides, examples and tests; no library
change.

- **The queue is a pattern, not a library feature.** It lives in
  `examples/work_queue/queue.rs`, which the example and
  `tests/work_queue.rs` both include, so the guide describes the code that is
  tested.
- **The fence is the attempt number.** Every write by a holder expects
  `job/state`, `job/owner` and `job/attempt`. Owner alone is not enough: one
  worker name can claim the same job twice. A mutation that removed the
  attempt expectation passed the first test suite, which led to a
  deterministic case and a property that keeps every claim a worker was ever
  given.
- **A takeover expects state, lease and attempt.** An expectation covers one
  attribute, so expecting the lease alone would let a takeover follow a late
  finish. The lease expectation makes it lose to a heartbeat.
- **Time alone takes nothing away.** An expired lease that nobody has taken
  still belongs to its owner. The store has no clock, so this is the only
  rule it can enforce.
- **A job holds a due time only while pending and a lease only while
  claimed.** A claim retracts `job/run-after`; finish, fail and release
  retract `job/lease-until`. The first form of the pattern retracted nothing,
  and its claim read was driven through every finished job: 6.5 ms at 8,000
  finished jobs against 0.6 ms now. The cost is one more event per claim and
  per finish, about 0.8 KB more file per job. `job/owner` is still the last
  claimant and stays on a finished job.
- **A holder's write also expects the lease it records,** so the retraction
  names the stored value. This makes a claim copied before a heartbeat fail
  like a lost one, and makes the state expectations on a holder's write and
  on a takeover redundant. They are kept as a second guard.
- **`claim` commits without rechecking the time;** `try_claim` checks it. A
  candidate that `claim` has just read for the same instant can only be
  refused by a commit, so the retry loop always makes progress. With the
  check inside the loop, a disagreement between the query and the check
  would spin forever.
- **A candidate carries why it was claimable,** and `try_claim` refuses one
  that is not claimable at the claimer's time. A heartbeat never shortens a
  lease.
- **Outside effects are keyed without the attempt number.** A key that
  includes it would let two attempts both take effect. The attempt is a
  fencing token, not part of an idempotency key.
- **Keys where a caller repeats without reading:** enqueue (from the job id)
  and finish (from the job and attempt). Claim and heartbeat read first and
  are fenced, so they carry no key and store no retry record.
- **The caller supplies the clock** in every function, so tests are
  deterministic and no assertion depends on elapsed time.
- **Agent conventions** are recorded in `docs/agents.md`: provenance and time
  in metadata on every commit, keys named by session and step, find-or-create
  for unique names, and one value in force held by an expectation on a
  single-valued attribute instead of supersession links.
- **Found, not decided for the library:** a read that joins attributes is
  driven from one of them and visits every entity holding it. The pattern
  now avoids this; other workloads may not be able to. Roadmap 12.
