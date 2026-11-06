# Incremental implementation plan

There is no delivery deadline. Correctness and reviewable increments determine
the pace. The full proposed v1 contract remains the target; a later item is
unfinished work, not an implicit removal from the contract.

Each step has a branch, implementation, validation, recorded self-review, and
merge. Large steps may use multiple bounded branches, each covering one or more
items. Every item closes with done, decisions, blockers, and what's next in
`status.md`. Reviews live in `reviews/`; no branch merges with known unmet checks.
Candidate work that is not yet authorized lives in [`roadmap.md`](roadmap.md)
and moves here as an item when approved.

## Step 0: Repository and development baseline

- [x] **0.1 Inspect and initialize.** Inspect existing files/instructions and
  tooling, establish a local `main`, and preserve unrelated workspace files.
- [x] **0.2 Record the contract and roadmap.** Preserve the handoff unchanged,
  document the working contract and workflow, break all steps into items, and
  review links, scope, and item dependencies before merge.
- [x] **0.3 Scaffold the workspace.** One Rust library crate and one CLI crate,
  explicit edition/toolchain, lockfile, unpublished packages, and no database
  API placeholders presented as working behavior.
- [x] **0.4 Establish verification.** One local check script and equivalent CI
  configuration for formatting, warnings, tests, and documentation. Run locally
  and review before merging. Clearly distinguish scaffolding checks from
  semantic acceptance evidence.

## Step 1: Semantic core

- [x] **1.1 Typed values and identity.** Define store-bound entity IDs,
  transaction-local labels, revisions/snapshots, typed values, finite-float
  validation, zero normalization, equality, and same-type ordering. Test type
  distinctions, i64 boundaries, invalid floats, and foreign identities.
- [x] **1.2 Schema and allocation state.** Immutable attribute definitions,
  cardinality/uniqueness rules, known empty entities, historical creation
  revisions, identical redeclarations, and transactional new definitions.
  Test unknown/empty entities and incompatible definitions.
- [x] **1.3 Request structure and builder.** Owned requests, metadata, new
  labels, assert/retract, expectations, head expectation, and optional retry
  key. Restrict metadata to typed scalars excluding references. Structural
  validation catches duplicate declarations and malformed requests without
  storage access. Build does not execute a transaction.
- [x] **1.4 Pure batch normalization.** Given pre-state and a validated request,
  calculate effective changes together. Test repeated operations, exact-fact
  add/retract conflicts, replacement, many-valued sets, and operation-order
  independence of final state. The reference state is a semantic oracle, not a
  public second backend.
- [x] **1.5 Constraints and reports.** Validate expectations against pre-state,
  references including forward references/cycles, and uniqueness against final
  state. Produce deterministic deltas, metadata, and failures with the design's
  stable error codes and structured context.
  Test atomic unique transfer, failed expectations, and no partial allocations.

Temporary allocation inputs to the pure evaluator must be explicit so it can be
tested deterministically without committing to a public ID encoding prematurely.

## Step 2: Atomic SQLite persistence

- [x] **2.1 Storage format and open.** Select and document SQLite binding,
  durability policy, busy timeout, format version, store/lineage identity,
  and revision-zero creation. Reject unsupported formats and inconsistent files.
- [x] **2.2 Authoritative log and replay.** Persist commit records, complete
  reports, schema/allocation events, and ordered effective fact events. Rebuild
  state through revision R and compare it with the pure evaluator.
- [x] **2.3 Serialized commit.** Begin a SQLite write transaction, validate
  against its head, evaluate the batch, persist all records, and commit together.
  Verify rollback and fresh no-change revisions. Reserve storage for versioned
  canonical requests; requests with retry keys must be rejected until step 4
  is complete, rather than silently committing without retry protection.
- [x] **2.4 Failure and concurrency checks.** Exercise failures around writes,
  competing connections, conditional-update contention, and busy classification.
  Distinguish confirmed rollback from uncertain outcomes.

Start with replayed state for reads and commit validation. A current projection
is deferred until justified by measurements; it must remain rebuildable from the
log and cannot become the historical authority.

## Step 3: Stable views and the A/B/reopen fixture

- [x] **3.1 Pinned `Db` ownership and tokens.** Capture head once, share durable
  access without retaining a long-lived SQL transaction, validate full snapshot
  identities and bounds, and keep reads usable after dropping `Store`.
- [x] **3.2 Revision-bounded reads.** Schema, facts, flat pull, history, and
  transaction enumeration all read the same basis. Test historical unknown
  attributes, known empty entities, omitted absent fields, deterministic value
  ordering, no future events, and no-change transaction records.
- [x] **3.3 A/B/reopen acceptance fixture.** Atomically create Ada/task/schema,
  capture A, conditionally replace status at B, reopen, and recover both views.
  Validate failed conditions, unrelated changes, and expected-head fencing.
- [x] **3.4 Reader/writer isolation.** Force a writer between dependent reads;
  every public operation must use one consistent SQL snapshot. If a current
  projection is later introduced, force the design's head-check/read race too.

## Step 4: Exact retries and the interchange boundary

- [x] **4.1 Versioned wire decoding.** Define Rust/JSON equivalence and strict
  parsing of tagged values, decimal integers/revisions, finite float strings,
  decoded bytes, defaults, unknown fields, and duplicate JSON keys.
- [x] **4.2 Canonical request representation.** Exclude the key, include version,
  metadata/head/schema/labels/operations, normalize sets/zeros/defaults, preserve
  operation order, and store canonicalization version plus exact bytes. Test
  alternate decimal/byte spellings and deliberate reorder conflicts.
- [x] **4.3 Atomic retry lookup.** Check key and canonical bytes inside the
  writer transaction before current schema/head/entity/expectation checks.
  Persist the original report and key atomically. Test stale replay, changed
  requests, concurrent same-key submissions, failure without key reservation,
  and exact original reports/IDs after reopen.
- [x] **4.4 Thin CLI and fixture parity.** Implement init/transact/pull/history/
  transactions/schema commands as needed, stdin/file input, versioned JSON output,
  structured stderr errors, and documented exit codes. Exercise the complete
  create/start/reopen/retry fixture in separate processes.

Versioned public JSON and durable canonical requests are intentionally designed
together. Storage internals may have an explicit internal encoding earlier;
they must not accidentally become an undocumented public wire contract.

## Step 5: Basic joined queries

- [x] **5.1 AST and historical type checking.** Patterns, typed inputs/constants,
  predicates, projection, order, and limit; validate bindings and ref/entity
  unification against the view's schema.
- [x] **5.2 Reference evaluation.** Match/filter/project/distinct/order/limit
  over replayed state. Test Ada/task joins, many-valued combinations, absent
  facts, type errors, duplicate tuples, zero limits, and total tie-breakers.
- [x] **5.3 Public query parity.** Shared Rust/JSON conformance fixtures and CLI
  query command reproduce the design's results at A and B.
- [x] **5.4 Bound SQL compilation.** Resolve attributes and bind values without
  string interpolation. Decide an explicit historical SQL strategy and compare
  its answers with reference evaluation before enabling it.

Replay-based historical reads require an explicit bridge to SQL querying. Choose
that bridge when implementing 5.4; current tables alone cannot serve past views.

## Step 6: Hardening and measured optimization

- [x] **6.1 Acceptance matrix audit.** Map every design acceptance row to tests
  and remaining gaps, including all value types, schema, identity, and uniqueness.
- [x] **6.2 Recovery and process behavior.** Multiple processes, crash/commit
  boundaries, bounded busy handling, corruption/format errors, and compatible
  reopen behavior. Validate projection rebuilding if a projection exists.
- [x] **6.3 Backup and lineage.** SQLite-safe backups, token-preserving restore,
  documented writer retirement, and explicit unsupported divergent clones.
- [x] **6.4 Representative measurements.** Establish workloads and performance
  needs, then choose indexes/checkpoints/projections. Compare every optimized
  path with replay, including historical schema and concurrent reads.

Item 6.4 establishes a measured baseline, with no invented production latency
budget. Existing indexes/replay remain; cache/checkpoint/projection selection is
deferred until an actual workload and budget are agreed. See `measurements.md`.

JMESPath, server/NIF integration, and all other deferred scope remain outside the
initial implementation. Licensing and remote destination are open repository
decisions; neither changes the local semantic implementation sequence.

## Step 7: First use, combined traces, and replay profiling

- [x] **7.1 First-use API pass.** Add a library guide and executable Rust
  workflow covering creation, pinned traversal, conditions, queries, exact
  retries, backup and reopen. Run the example in the check script; fix concrete
  usability issues found by using the public API.
- [x] **7.2 Combined stateful testing.** Compare mixed randomized operation
  traces with an independent state oracle: schema, references, uniqueness,
  conditions, retries, historical reads and reopen. Retain shrinking and at
  least 1,024 property cases.
- [x] **7.3 Replay profiling.** Measure replay stages, choose one demonstrated
  improvement, and verify optimized/reference parity, corruption detection,
  rollback, isolation and recovery. Report observed costs without inventing an
  SLA or adding an unmeasured cache/checkpoint.

## Step 8: Handoff and remote preservation

- [x] **8.1 Prepare and verify the handoff.** Record restore/resume instructions;
  package source, locked dependencies, reviews and complete reachable Git history
  with branch names. Exclude machine state. Restore independently and compare
  commit, tree, branch heads and tracked bytes; check Git integrity and run the
  public Rust workflow from the restored checkout.
- [x] **8.2 Publish and verify.** Push to the selected remote without replacing
  unrelated history, compare remote refs with local refs and verify a fresh
  remote clone. Deliver the final archive, manifest and SHA-256 checksum for
  the exact published commit. Report remote CI separately from local evidence.

## Step 9: Indexed temporal state

Measurements in 6.4 and 7.3 show per-operation log replay dominating reads and
commits as retained history grows. This step adds derived, verifiable temporal
state without changing semantics. The log stays authoritative.

- [x] **9.1 Temporal datom index.** Add a format-2 `datoms` table holding one
  validity interval `[asserted, retracted)` per effective assertion, maintained
  in the same SQLite transaction as each commit. Migrate format-1 files on
  writable open. On every open, verify the table equals the intervals derived
  from retained fact events, after full log validation. Load the state at R for
  reads and commit evaluation from schema/entity events and the interval table
  rather than replaying the log. Test index/replay parity for every revision of
  randomized typed histories, migration, tampered or missing intervals, rollback
  leaving no interval rows, and the forced writer interleaving for every read.
- [x] **9.2 Direct indexed reads.** Answer `facts`, `pull` and `query` with
  revision-bounded SQL over the interval table, without loading the complete
  state at R. Compare every path with the reference evaluator over replayed
  state, including historical schema and the forced writer interleaving.
  Format 3 adds the attribute/value index this needs; writable opens upgrade
  formats 1 and 2.
- [x] **9.3 Open without the write lock.** Validate an existing current-format
  file in a deferred read transaction. Take the write lock only to initialize an
  empty file or upgrade format 1, rechecking the file once it is held. Test that
  open succeeds promptly while another connection holds the write lock, that
  concurrent first opens create one identity, and that initialization and
  commits still fail `Busy` within a bound.
- [x] **9.4 Incremental replay validation.** Chosen by the user on 2026-11-05.
  Make each transition's checks visit only the facts it touches, keeping
  every invariant open validates. Retain the full-state transitions in test
  builds and require identical states, reports and errors on random batches
  and tampered reports, plus replay parity and the existing tampering cases.
- [x] **9.5 Commits load only touched state.** Evaluate a commit against the
  schema, the identities the request names, the next allocation, the visible
  values of every entity/attribute it changes or expects, and the owners of
  every unique value it asserts. Require identical reports and errors to
  full-state evaluation and to the committed result on random requests.

## Step 10: Babashka pod

Authorized by the user on 2026-11-05, for the clajent agent kit: durable,
queryable agent-run records with history from Babashka. The pod is a thin
adapter. Storage semantics stay in the library and validation stays in the
strict wire decoders.

- [x] **10.1 Pod crate.** Add `factlite-pod` (binary `pod-factlite`) on the
  published `babashka-pod` crate, using transit+json. Expose open/close, head,
  transact, schema, pull, facts, history, transactions and query, with optional
  `:at` snapshots or revisions for reads. Translate Clojure data to the version-1
  JSON wire requests and back, so the pod reuses the strict wire validators. Each
  of the six value types maps to one Clojure type and back without loss.
  Anything else is rejected, never coerced. Return every library error as
  `ex-info` data with its stable code, context and commit outcome. Test the
  translation in Rust, run a babashka script over stdio and socket transports,
  and run several pod processes committing to one file concurrently with no lost
  or merged writes.

## Step 11: Consumer requests

Requests from the clajent kit's pod trials, authorized by the user on
2026-11-05. Aggregates and new value types were deferred in the original
design and need a separate decision; they are not in this step.

- [x] **11.1 Open options and read-only pod connections.** A validated
  busy-timeout option for writable and read-only opens. Explicit refresh of a
  read-only view to the latest head or a retained snapshot over its own
  connection. Pod `open` accepts `:read-only` and `:busy-timeout-ms`.
- [x] **11.2 Lookup refs for reads.** Resolve an entity by a unique attribute's
  value at a view's revision, and accept `[attr value]` in pod read positions.
- [x] **11.4 Aggregates.** Chosen by the user on 2026-11-05, extending the
  original design's query scope. count, count-distinct, sum, min and max over
  distinct projected tuples, grouped by the selected variables, with an
  explicit `with` list. Shared exact arithmetic for SQL and reference paths;
  wire, CLI and pod surfaces; independent-oracle and parity properties.
- [x] **11.5 Keyword and inst value types.** Chosen by the user on
  2026-11-05. Explicit validation, equality, ordering and encoding contracts;
  storage tags 6 and 7 with a format-4 table rebuild; wire, canonical, query
  and pod mappings; every typed property extended to eight types.
- [x] **11.3 Indexed filtered history.** Answer entity- or attribute-filtered
  history from datom intervals and per-revision events instead of decoding
  every retained report, with parity against the log path.
