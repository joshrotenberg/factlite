# Factlite design for a SQLite backed fact database

## Purpose and status

Factlite is an embedded Rust library that gives SQLite a typed fact database API: atomic assertions and retractions, explicit historical database views, and small relational queries over facts. A thin CLI exercises the same library. A future Elixir server through a NIF is a possible consumer, not part of the first build.

This document is a design handoff for a fresh Codex session. It contains no implementation or evidence that Factlite has been built or tested. Rust signatures, JSON, commands, and results below are illustrative contracts to implement and test later. Implementation should begin only when separately requested.

The working baseline established in discussion is:

- A writable `Store` and stable, revision-pinned `Db` views, backed by retained history rather than long-lived SQLite transactions.
- Typed EAV facts, cardinality one or many, explicit assertion/retraction, atomic preconditions, temporary IDs, transactional schema additions, and structured reports.
- Successful no-change transactions receive revisions; exact idempotent retries return the original transaction.
- A small typed query AST shared by Rust and JSON, with conjunctive joins, comparisons, distinct projection, ordering, and limits.

**Recommended decisions** below resolve edge cases that were not individually settled in discussion. They are the proposed v1 contract, visibly distinguished from the baseline. API spelling and physical table layouts remain adjustable without changing those semantics.

The first proof of usefulness is small: create a person and related task atomically, query them, conditionally change the task, close and reopen the store, and recover the same answers at the earlier revision.

## The data model

A fact is `(entity, attribute, value)`. At a given revision, facts form a set. The log records effective additions and retractions, associated with the transaction that produced them.

### Entities and references

`EntityId` is an opaque, store-local identity, not a user-supplied integer or business key. Callers obtain it from allocation, a read, or a transaction report. `TempId` is a transaction-local label used to create related entities in one atomic request. Every temporary label must be declared once; undeclared references are errors. Two labels always create two entities, even if they assert identical facts.

**Recommended:** maintain a durable entity allocation catalog, including creation revision. An allocated entity remains known after its last fact is retracted; creating an empty entity is valid. This makes identity independent of current attributes. An ID unknown at a view's revision is not a known empty entity. Writes using unknown IDs fail, including retractions; retracting an absent fact on a known entity succeeds as a no-op.

Reference values are explicitly typed. An assertion may reference an entity already allocated in this store or a declared temporary entity in this transaction. Forward references and cycles are valid. Reject fabricated or foreign targets. There is no entity deletion operation or automatic cascade in v1. References may point to allocated entities with no current facts; that is a valid empty target, not a dangling identity. Applications can model archival or tombstones with attributes.

Public ID parsers validate syntax; operations also validate identity membership. Serialized entity IDs bind store identity as well as an opaque allocation component; operations validate both the binding and allocation membership. Identical-looking local numbers from different stores never identify the same entity.

### Values and equality

Proposed initial values are `Bool`, `I64`, `F64`, `String`, `Bytes`, and `Ref(EntityId)`. Absence is represented by no fact. There is no stored null, embedded object, or nested array value. Add timestamp, decimal, UUID, and other types only with explicit comparison and encoding contracts.

**Recommended equality rules:**

- Values include their type tag: `I64(1)`, `F64(1.0)`, `String("1")`, and an entity reference are distinct.
- Reject NaN and positive/negative infinity at every input boundary. Normalize negative zero to positive zero before equality, hashing, persistence, and retry comparison.
- Strings compare exactly, without Unicode normalization or case folding. Bytes compare byte-for-byte.
- Ordering is numerical within `I64` or finite `F64`, byte-lexicographic for strings and bytes, and deterministic by opaque ID encoding for references. Booleans order false before true. Cross-type ordering and implicit numeric coercion are errors.

SQLite affinity and collation must not silently redefine these rules.

### Attributes and schema

Each attribute has an immutable qualified name, value type, cardinality (`one` or `many`), and optional uniqueness (`none` or `unique_one`). Cardinality-many values are sets, not ordered lists. `unique_one` requires cardinality one and rejects the same typed value on two entities. It never merges entities or performs identity upserts.

Attributes are added transactionally and may be used by facts in that same transaction. Definitions become visible only at the committed revision. A historical view resolves names and validates queries against its historical schema. An attribute added later is unknown there, even if the current store recognizes it.

**Recommended:** freeze name, type, cardinality, and uniqueness for v1; no rename, removal, or migration of existing definitions. Redeclaring an identical definition is an effective schema no-op; a different definition with that name is a conflict. New definitions can be specified at most once per request. Physical index changes are storage maintenance and cannot alter historical semantics.

There is no required entity shape, class membership, or mandatory field validation. Domain validation belongs to the application. Metadata does not implicitly become entity facts.

## Store and stable database views

`Store` owns durable access and performs commits. `Store::db()` captures the committed head once and returns a `Db` pinned to it. `Store::db_at(snapshot)` opens a particular historical view. A `Db` cannot write or silently refresh.

A `SnapshotId` contains an encoding version, store identity, lineage identity, and revision. Revisions start at zero for an empty store and increase once per successful new transaction. Revision numbers are logical commit order, not wall-clock time. Revision zero has no user entities or attributes.

Validate the full snapshot identity. Reject a wrong store, wrong lineage, nonexistent/future revision, or unavailable history. Never clamp to head or silently substitute another revision.

The `Db` owns a shareable store handle and snapshot token, not a borrowed SQL transaction. Its schema, name resolution, query planning, fact reads, and metadata visibility all use the same revision. Dropping the writable handle need not invalidate existing reads; closing all handles and reopening the retained file must still permit the same snapshot. A token is not a backup and cannot reconstruct a lost file.

**Recommended:** retain all committed history in v1. No pruning or compaction that discards semantic history. Use short SQLite transactions within individual operations as needed; retain no SQLite read transaction for the lifetime of a `Db`.

A read view is not a write fence. Reading A and then committing a request writes against the current head unless that request explicitly supplies expectations. Offer both exact attribute-set expectations and a full expected-head snapshot. The former allows unrelated concurrent changes; the latter rejects any intervening commit, including a successful no-change commit.

**Recommended copy rule:** supported read-only backups preserve tokens. A faithful disaster restore may resume the same lineage only with identical retained committed history and the previous writer retired. Divergent writable clones and continuing from an older backup after losing later commits are unsupported in v1. Arbitrary file copying cannot reliably be detected, so callers must uphold this operational restriction; do not claim automatic clone detection. Keep the lineage field now, but defer any relineaging operation until it defines how entity identities and persisted reports behave. Branching, merging, and lineage migration are outside v1.

## Transactions

A `Tx` is serializable data containing schema additions, temporary declarations, assertions/retractions, expectations, metadata, an optional expected head, and an optional idempotency key. A Rust builder produces the same request as the CLI. It does not execute statements incrementally.

### Declarative batch semantics

All expectations inspect the committed pre-transaction head. All writes are interpreted together. Request order never turns a conflicting batch into sequential updates.

- Asserting an existing fact or retracting an absent fact is an effective no-op.
- Repeated identical assertions or retractions collapse for transaction semantics.
- Asserting and retracting the exact same fact in one batch is a conflict, even if one side would be a no-op.
- Cardinality one permits at most one distinct asserted value per entity/attribute in a batch. If that value differs from the old value, the effective delta retracts the old value and asserts the new value.
- An explicit retraction of that old value plus assertion of its replacement is valid; the effective retraction appears once.
- Cardinality many allows independent additions and removals under the same attribute, subject to the exact-fact conflict rule.
- Validate uniqueness over the resulting complete state, allowing an atomic transfer of a unique value if the old owner's value is removed. Do not reject a valid transfer merely because of statement execution order.

Schema conflicts, typing errors, unknown entities, failed expectations, and uniqueness violations abort the entire request. No partial facts, metadata, ID allocations, or idempotency entry survive an abort. Uncommitted internal IDs need not be contiguous and must not be exposed as committed IDs.

### Preconditions and conditional updates

**Recommended:** `expect(entity, attribute, values)` means exact typed set equality at the pre-transaction head. `[]` means the attribute is absent. Duplicates in expected sets collapse. Cardinality-one expectations accept zero or one distinct value; many-valued expectations can contain multiple values. An expectation refers to an existing entity and an attribute visible at the pre-transaction head, not a newly declared entity or attribute.

A conditional one-valued update is `expect(e, a, [old])` plus `assert(e, a, new)`. `cas` may be convenience sugar for that request, including expected absence. Do not implement CAS as an unlocked read followed by a write. On failure, callers decide whether to reread and formulate a different request; the library never silently recomputes their intent.

### Commit and retry identity

The commit sequence is:

1. Parse the versioned request and perform schema-independent structural validation. Produce canonical typed request bytes.
2. Begin one SQLite write transaction and serialize against other writers.
3. Look up the idempotency key in this lineage. If found, compare canonical requests and return the persisted original report on a match. A mismatch returns `IdempotencyConflict`. Do this before current-head, schema, entity, or expectation validation.
4. For a new request, capture the current head; validate expected head, resolve the applicable schema, validate expectations and normalize the declarative batch.
5. Allocate temporary entities, validate resulting references/cardinality/uniqueness, and compute effective changes.
6. Assign the next revision. Atomically persist the commit record, metadata, schema/entity events, effective fact delta, current projection, full report, and optional idempotency entry; then commit SQLite.

A valid new request receives a revision even if it changes no facts, schema, or entities. An idempotent replay creates no revision and returns its original before/after snapshots, IDs, metadata, and delta even if head has advanced. Failed requests do not reserve keys.

**Recommended canonicalization v1:** compare the submitted typed request, not expanded retractions, resolved attribute IDs, allocated entity IDs, or current state. Exclude the key itself; include metadata, expectations, expected head, schema, temporary labels, all operations, and request version. Sort object keys and normalize set-valued expectations. Encode typed integers in one canonical signed-integer representation and floats as normalized IEEE 754 binary64 bits, so decimal spellings cannot affect retry equality. Decode bytes before canonical comparison; the wire encoder always emits padded standard base64. Preserve operation-array order and temporary labels. Thus a reordered but semantically equivalent batch can conflict under the same key; alpha-renaming temporary labels can too. State this explicitly to callers.

Store canonicalization version and canonical bytes with the committed report; a digest is an accelerator, not the sole equality evidence. Retain support for existing canonicalization versions across compatible upgrades. A caller must retry the same request and key after an ambiguous timeout or lost response. An exhausted SQLite busy timeout is retryable; distinguish confirmed rollback from an unknown commit outcome.

### Reports and failures

`TxReport` contains `before`, `after`, temporary-ID mappings, created entities, added schema definitions, effective fact delta, and persisted caller metadata. Server-assigned transaction details may include a timestamp, but timestamps do not determine ordering. Delta order is deterministic: retractions precede assertions, then entity/attribute/typed-value order. History preserves this order using an event ordinal.

Errors have stable codes and structured context: `InvalidRequest`, `UnknownAttribute`, `TypeMismatch`, `UnknownEntity`, `ConflictingBatch`, `UniqueViolation`, `PreconditionFailed`, `HeadMismatch`, `IdempotencyConflict`, `InvalidSnapshot`, `Busy`, `Storage`, and `UnsupportedFormat`. A failed expectation identifies the entity/attribute and expected versus actual typed sets. Rust retains source errors; CLI consumers do not parse prose for control flow.

## Reads and queries

All read results are owned values. No public result depends on a live SQL cursor or an implicit mutable cache.

- `facts(filter)` returns current effective facts at the `Db` snapshot.
- `pull(entity, attributes)` returns a flat typed entity projection. Cardinality-one fields contain one typed value; many-valued fields contain deterministically ordered arrays. Absent fields are omitted. A known empty entity returns its ID with empty attributes; an entity not allocated at this revision returns `None`.
- `history(filter, after_revision)` returns effective fact events in `(after_revision, db.revision]`, ordered by revision and event ordinal. No-op assertions/retractions do not appear as fact events.
- `transactions(after_revision)` returns committed transaction records in the same bounded interval, including no-change commits and metadata. This is the foundation for a later change feed, not a live subscription.
- `schema()` returns definitions visible at this revision. References returned by pull are typed IDs; any later traversal must explicitly reuse the same `Db`.

Reject a lower bound greater than the view's revision. No unbounded read may accidentally include a later commit.

### Initial query contract

A query is a typed AST with `inputs`, conjunctive `match` patterns, comparison predicates, `select`, optional `order`, and optional `limit`. Attribute positions are fixed names. Entity/value positions may contain variables, typed constants, or typed parameters. Variables must be bound by patterns; reject unbound projections or comparisons.

Shared variables join matching facts. A variable may unify an entity position with a `Ref` value position. It may not unify either with a scalar integer or string. Infer variable types from the historical schema and reject incompatible uses before execution. Unknown attributes are errors, not empty relations. Reject duplicate selected column names, undeclared or missing parameters, and negative limits; a zero limit returns no rows. Missing facts simply fail to match. Multiple many-valued patterns produce combinations before projection.

Evaluation is: match and filter, project selected tuples, remove duplicate tuples, order, then limit. Initial comparisons are equality/inequality and same-type ranges. A selected entity-domain value is returned as a typed reference. Initial results are a relation of named columns and typed rows, not automatically nested entities.

Only selected fields may appear in `order`. Without an order, row order is unspecified. For predictable limits, include an explicit total tie-breaker over the distinct selected tuple, often entity ID. The implementation must not pretend a non-unique sort key gives stable page boundaries. Offset/keyset pagination is deferred; a future cursor must bind snapshot, query, parameters, and ordering.

Compile the AST to bound SQL, with schema-resolved attribute IDs and explicit value tags. Do not interpolate user strings as SQL. Cache keys must include the relevant historical schema basis, query shape, and encoding version; current-head schema caches cannot serve historical views blindly.

### JMESPath remains an exploration

JMESPath is worth evaluating because of its functional-style JSON slicing. A natural experiment is shaping an already materialized result into application JSON. That is a candidate, not a commitment that it is only useful there or that it will be Factlite's query language.

Before adoption, compare its JSON/null/number semantics with Factlite's typed values and consider which predicates, if any, can be pushed into indexed fact queries. Filtering a limited result page is not equivalent to database filtering before pagination. Keep the core query and transaction semantics independent while evaluating it. The [JMESPath specification](https://jmespath.org/specification.html) defines the language to test against.

## Conceptual Rust surface

Names are illustrative; error plumbing, generics, and constructors are shortened. `TxBuilder` should own its request and validate local structure without borrowing a SQLite transaction.

```rust
pub struct Store { /* shared durable store handle */ }
pub struct Db { /* shared handle + immutable SnapshotId */ }
pub struct SnapshotId { /* store, lineage, revision */ }
pub struct EntityId { /* opaque store-bound identity */ }
pub struct TempId { /* stable request-local label */ }

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self>;
    pub fn db(&self) -> Result<Db>;
    pub fn db_at(&self, at: &SnapshotId) -> Result<Db>;
    pub fn commit(&self, tx: Tx) -> Result<TxReport>;
}
impl Db {
    pub fn snapshot(&self) -> &SnapshotId;
    pub fn schema(&self) -> Result<Schema>;
    pub fn facts(&self, filter: FactFilter) -> Result<Vec<Fact>>;
    pub fn pull(&self, id: &EntityId, attrs: &[AttrName])
        -> Result<Option<Entity>>;
    pub fn query(&self, query: &Query, params: &Params)
        -> Result<QueryResult>;
    pub fn history(&self, filter: FactFilter, after: Revision)
        -> Result<Vec<FactEvent>>;
    pub fn transactions(&self, after: Revision) -> Result<Vec<TxRecord>>;
}
```

A builder's essential vocabulary is `define_attribute`, `new_entity(label)`, `assert`, `retract`, `expect`, `expect_head`, `metadata`, `idempotency_key`, and `build`. Commit is separate. Avoid an ambiguous whole-entity `save` that might erase unspecified fields. Keep JSON serialization and the CLI outside the storage semantics; begin with one library crate and one CLI crate rather than a plugin/backend framework.

## Wire format and worked example

The proposed wire format is versioned JSON. Every value has exactly one type tag, for example `{"i64":"9223372036854775807"}`, `{"f64":"1.5"}`, `{"bytes":"AQI="}`, `{"string":"todo"}`, `{"bool":true}`, or `{"ref":{"id":"e1"}}`. Integers/revisions are decimal strings to avoid JSON consumer precision loss. Floats use finite round-trippable decimal strings; bytes use canonical padded base64. Temporary references use `{"ref":{"temp":"person"}}` only in transaction input. Entity positions use `{"id":"e1"}` or `{"temp":"task"}` and are never ordinary value strings.

Unknown fields, duplicate JSON object keys, malformed tags, overflow, and nonfinite floats are errors. Metadata is initially a string-keyed map of typed scalar values, excluding references. Omitted optional request fields have explicit defaults: empty collections, empty metadata, and no key/head expectation. Canonicalization inserts those defaults.

The fixture below uses opaque store ID `S`, lineage `L`, and shortened allocated IDs `e1` and `e2` for readability. Actual entity tokens include their store binding and are generated by the implementation; callers copy them from reports. The snapshot object is the portable token representation, with an encoding version, opaque identity strings, and a decimal revision. The commands describe the proposed CLI, not an existing binary.

### Create the graph at A

Save as `create.json`:

```json
{
  "version": 1,
  "idempotency_key": "demo-create-v1",
  "schema": [
    {"name":"person/name","type":"string","cardinality":"one","unique":"none"},
    {"name":"task/title","type":"string","cardinality":"one","unique":"none"},
    {"name":"task/status","type":"string","cardinality":"one","unique":"none"},
    {"name":"task/owner","type":"ref","cardinality":"one","unique":"none"}
  ],
  "new_entities": ["person", "task"],
  "expect": [],
  "changes": [
    {"op":"assert","entity":{"temp":"person"},"attr":"person/name","value":{"string":"Ada"}},
    {"op":"assert","entity":{"temp":"task"},"attr":"task/title","value":{"string":"Design Factlite"}},
    {"op":"assert","entity":{"temp":"task"},"attr":"task/status","value":{"string":"todo"}},
    {"op":"assert","entity":{"temp":"task"},"attr":"task/owner","value":{"ref":{"temp":"person"}}}
  ],
  "metadata": {"source":{"string":"design-example"}}
}
```

```sh
factlite init demo.sqlite
factlite --db demo.sqlite transact --file create.json > A.report.json
jq '.after' A.report.json > A.json
jq -r '.tempids.task' A.report.json > task.id
```

A's report has before `{ "version":1, "store":"S", "lineage":"L", "revision":"0" }`, after `{ "version":1, "store":"S", "lineage":"L", "revision":"1" }`, mappings `person -> e1`, `task -> e2`, both created entities, the four schema definitions, four effective assertions, and the submitted metadata. No temporary label appears in persisted facts.

### Move the task to doing at B

Save as `start.json`, substituting the task ID from A in a real session:

```json
{
  "version": 1,
  "idempotency_key": "demo-start-v1",
  "expect": [
    {"entity":{"id":"e2"},"attr":"task/status","values":[{"string":"todo"}]}
  ],
  "changes": [
    {"op":"assert","entity":{"id":"e2"},"attr":"task/status","value":{"string":"doing"}}
  ],
  "metadata": {"source":{"string":"design-example"}}
}
```

```sh
factlite --db demo.sqlite transact --file start.json > B.report.json
jq '.after' B.report.json > B.json
```

The full report for this fixture is:

```json
{
  "version": 1,
  "before": {"version":1,"store":"S","lineage":"L","revision":"1"},
  "after": {"version":1,"store":"S","lineage":"L","revision":"2"},
  "tempids": {},
  "created_entities": [],
  "schema_added": [],
  "effective_delta": [
    {"op":"retract","entity":{"id":"e2"},"attr":"task/status","value":{"string":"todo"}},
    {"op":"assert","entity":{"id":"e2"},"attr":"task/status","value":{"string":"doing"}}
  ],
  "metadata": {"source":{"string":"design-example"}}
}
```

### Read and join at explicit revisions

```sh
factlite --db demo.sqlite pull --at-file A.json --entity e2 --attrs task/status
factlite --db demo.sqlite pull --at-file B.json --entity e2 --attrs task/status
factlite --db demo.sqlite history --at-file B.json --entity e2 --attr task/status --after 0
```

The two pulls return respectively:

```json
{"version":1,"entity":{"id":"e2"},"attributes":{"task/status":{"string":"todo"}}}
```

```json
{"version":1,"entity":{"id":"e2"},"attributes":{"task/status":{"string":"doing"}}}
```

History contains assertion of `todo` at revision 1, then retraction of `todo` and assertion of `doing` at revision 2. History at A contains only the first event. These commands run in separate processes, making reopen behavior part of the intended fixture.

Save as `query.json`:

```json
{
  "version": 1,
  "query": {
    "inputs": {"wanted":"string"},
    "match": [
      [{"var":"task"},"task/status",{"param":"wanted"}],
      [{"var":"task"},"task/title",{"var":"title"}],
      [{"var":"task"},"task/owner",{"var":"person"}],
      [{"var":"person"},"person/name",{"var":"name"}]
    ],
    "where": [],
    "select": ["task","title","name"],
    "order": [{"field":"title","direction":"asc"},{"field":"task","direction":"asc"}],
    "limit": 20
  },
  "params": {"wanted":{"string":"doing"}}
}
```

```sh
factlite --db demo.sqlite query --at-file A.json --file query.json
factlite --db demo.sqlite query --at-file B.json --file query.json
```

At A the result is `{"version":1,"columns":["task","title","name"],"rows":[]}`. At B it is:

```json
{
  "version": 1,
  "columns": ["task","title","name"],
  "rows": [[{"ref":{"id":"e2"}},{"string":"Design Factlite"},{"string":"Ada"}]]
}
```

### Retry and conflict outcomes

Retrying `start.json` with `demo-start-v1` returns exactly B's persisted report even though `todo` is no longer current. No new revision is created. Retrying `create.json` likewise returns A's report without allocating anything again. Reusing either key with changed metadata or content returns `IdempotencyConflict`.

Submitting the B request under a new key now fails `PreconditionFailed`, with expected `[{"string":"todo"}]` and actual `[{"string":"doing"}]`. Head stays B. A separate unconditioned assertion of `doing` under a new key succeeds at revision 3 with an empty fact delta. A remains `todo`, B remains `doing`, and a later retry of B still returns revision 2.

The CLI supports stdin via `--file -`, emits one versioned JSON result on stdout, and sends JSON error objects plus any diagnostics to stderr with a nonzero exit code. Avoid progress chatter on stdout. Proposed exit codes are 0 success, 2 invalid input, 3 conflict/precondition, 4 busy, and 5 storage failure. The library owns classification; the CLI owns rendering. Human-friendly projections may be added separately, without changing lossless interchange.

## SQLite storage approach

SQLite supplies local transactional durability, locking, indexing, and recovery. Factlite must supply temporal meaning, type safety, normalization, and stable views. SQLite serializes writers; its transaction snapshots are not substitutes for durable application-level historical tokens. See [SQLite isolation](https://www.sqlite.org/isolation.html).

Start with an append-only semantic log and an atomically maintained, rebuildable current projection. A conceptual layout is:

- `store_info`: format version, store identity, lineage, committed head.
- `transactions`: revision, metadata, request/canonicalization version, complete report, optional key and canonical bytes.
- `schema_events` and `entity_events`: durable definition/allocation changes by revision.
- `fact_events`: revision, ordinal, operation, entity, attribute, type tag, canonical value.
- `current_schema`, `current_entities`, and `current_facts`: indexed derived state at head.

These are design responsibilities, not prescribed DDL. Choose value encodings that preserve exact typed equality and supported order; use explicit type tags, never an untyped `value` column with accidental coercions. Backstop cardinality and uniqueness with constraints where useful, while evaluating final-state constraints correctly for batch transfers.

The simplest correctness oracle reconstructs schema/entities/facts by replaying events through revision R. Historical reads can initially use this approach, even if slower. For projection reads, establish one short SQLite read transaction, check that its visible head equals R, and perform all schema, entity, fact, and query reads for that operation in the same SQLite snapshot. If its visible head is newer, reconstruct R instead. A head check outside the read transaction is insufficient: a writer could commit between the check and the reads. Never read a newer projection and merely attach an old token.

Optimize later with checkpoints, validity intervals, or indexed historical SQL only when they produce the same answers as replay. Publish new cache/projection state only after commit. On reopen, verify format and projection/head consistency; fail clearly or rebuild derived state from authoritative events. Unsupported newer formats fail closed. Migrations must preserve revisions, identity, reports, and historical schema behavior.

Use an explicitly configured durability policy, bounded busy timeout, and SQLite-safe backup mechanisms. WAL may aid concurrent local reads but introduces sidecar files; do not promise that copying only the main file during use is a consistent backup. No direct external SQL writes to Factlite-owned tables are supported. Do not introduce a generic backend abstraction or network filesystem promise in the first build.

## Delivery phases and acceptance tests

### First useful slice

Implement only after authorization: typed core model, store creation/open, schema additions, entity allocation, atomic assert/retract, expectations, idempotency, reports, replay-based historical reads, flat pull, and the thin CLI. Deliver the complete A/B/reopen/retry fixture before expanding query syntax. A small in-memory reference model is useful as an executable semantic oracle, not a second storage backend.

### Basic joined queries

Add the bounded AST, type checking, inner joins, distinct projection, order, and limit. Keep Rust and CLI conformance fixtures identical. Introduce SQL compilation while comparing results with the reference evaluator.

### Hardening before optimization

Test multiple connections/processes, busy behavior, crash boundaries, projection rebuild, format rejection, and backup/restore lineage rules. Only then measure representative current and historical workloads and choose indexes or checkpoints. No performance target has yet been established.

| Area | Required acceptance evidence |
| --- | --- |
| Atomicity | Failure at every commit stage leaves either the full old state or full new state, including report/key/metadata |
| Stable views | A and its schema give identical reads and query results before B, after B, and after reopen; force a writer commit between projection head-check and reads to prove one SQL snapshot is used |
| Set semantics | Duplicate assertions and absent retractions are no-ops; fresh no-change commits still advance revision |
| Conflicts | Same-fact add/retract and multiple one-valued assertions abort regardless of ordering |
| Conditional writes | Two competing updates from `todo` cannot both succeed; unrelated changes pass attribute expectations but fail expected-head fencing |
| Retry safety | Concurrent identical keys commit once; replay bypasses stale expectations; changed requests conflict; original IDs/reports survive reopen |
| Identity | Empty entities, cycles, forward temporary references, unknown IDs, and foreign IDs follow the stated catalog rules |
| Schema | Same-transaction definitions work; historical unknown attributes remain unknown; incompatible redeclarations abort |
| Uniqueness | Duplicate ownership aborts; valid atomic transfer succeeds without identity merging |
| Typed values | Full i64 range, finite floats, zero normalization, strings, bytes, and references round-trip and compare consistently |
| Queries | Ref/entity joins, absent attributes, many-valued combinations, distinct tuples, type errors, and ordered limits match the reference evaluator |
| History | Upper bounds include no future events; transaction enumeration includes no-change commits; fact history contains only effective changes |
| Recovery | Rebuilt projections equal replay; unsupported formats fail; supported disaster restore preserves identical history; no supported path silently continues divergent history |
| CLI parity | Typed JSON and Rust requests produce equivalent reports/errors; stdout remains machine-readable |

These are acceptance requirements, not completed tests. Favor semantic property tests and small deterministic fixtures over a broad feature demo.

## Scope boundaries and precedents

Do not add full Datalog, recursion, optional/negative patterns, aggregates, custom query/transaction functions, dynamic attribute variables, nested recursive pull, identity upserts, cascades, valid-time/bitemporal semantics, branching/merging/sync, subscriptions, a server, or NIF bindings to the initial scope. Each can change correctness or operational commitments significantly.

The precedents inform the API without implying compatibility:

- [Datomic's client API](https://docs.datomic.com/client-api/datomic.client.api.html) provides explicit database values, as-of/history surfaces, pull/query, and transaction reports with before/after values and temporary-ID mappings. Borrow the clarity of those boundaries; do not claim its `since` view is a complete change feed.
- [DataScript](https://github.com/tonsky/datascript) is useful for its immutable database/query model; [Datahike](https://github.com/replikativ/datahike) and [Datalevin](https://github.com/datalevin/datalevin) provide persistent Datalog-family comparisons. They are references, not dependencies selected for Factlite.
- [XTDB](https://docs.xtdb.com/) is an important temporal-database comparison. Its current SQL/bitemporal scope is broader than Factlite's proposed single commit-time axis; do not conflate current XTDB with its older API generation.
- [Mentat](https://github.com/mozilla/mentat) is the closest historical Rust/SQLite precedent. The prior source inspection used commit [`e55376e98b3a7f841dcb2abd6273947f8661ffc8`](https://github.com/mozilla/mentat/tree/e55376e98b3a7f841dcb2abd6273947f8661ffc8): current datoms plus a typed assertion/retraction log, schema/index maintenance, and a substantial query compiler. Its examined read surfaces did not establish Factlite's promised durable as-of `Db` contract. The lesson is to pin historical schema and query basis deliberately, and contain transaction/query complexity. Mozilla's [September 2018 closure explanation](https://groups.google.com/g/firefox-dev/c/1mAQw1aWPTw) cites delivery deadlines and product integration priorities; it is not evidence that EAV on SQLite fundamentally failed. Mentat's archive status should neither veto Factlite nor justify adopting its complete architecture.

The central decision is to make durable historical correctness part of the first useful build. Keep the interface small enough that its fact semantics, retries, schema visibility, and query results can be explained by the same reference model.
