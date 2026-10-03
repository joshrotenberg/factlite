# Incremental implementation plan

There is no delivery deadline. Correctness and reviewable increments determine
the pace. The full proposed v1 contract remains the target; a later item is
unfinished work, not an implicit removal from the contract.

Each step has a branch, implementation, validation, recorded self-review, and
merge. Large steps may use multiple bounded branches, each covering one or more
items. Every item closes with done, decisions, blockers, and what's next in
`status.md`. Reviews live in `reviews/`; no branch merges with known unmet checks.

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

- [ ] **3.1 Pinned `Db` ownership and tokens.** Capture head once, share durable
  access without retaining a long-lived SQL transaction, validate full snapshot
  identities and bounds, and keep reads usable after dropping `Store`.
- [ ] **3.2 Revision-bounded reads.** Schema, facts, flat pull, history, and
  transaction enumeration all read the same basis. Test historical unknown
  attributes, known empty entities, omitted absent fields, deterministic value
  ordering, no future events, and no-change transaction records.
- [ ] **3.3 A/B/reopen acceptance fixture.** Atomically create Ada/task/schema,
  capture A, conditionally replace status at B, reopen, and recover both views.
  Validate failed conditions, unrelated changes, and expected-head fencing.
- [ ] **3.4 Reader/writer isolation.** Force a writer between dependent reads;
  every public operation must use one consistent SQL snapshot. If a current
  projection is later introduced, force the design's head-check/read race too.

## Step 4: Exact retries and the interchange boundary

- [ ] **4.1 Versioned wire decoding.** Define Rust/JSON equivalence and strict
  parsing of tagged values, decimal integers/revisions, finite float strings,
  decoded bytes, defaults, unknown fields, and duplicate JSON keys.
- [ ] **4.2 Canonical request representation.** Exclude the key, include version,
  metadata/head/schema/labels/operations, normalize sets/zeros/defaults, preserve
  operation order, and store canonicalization version plus exact bytes. Test
  alternate decimal/byte spellings and deliberate reorder conflicts.
- [ ] **4.3 Atomic retry lookup.** Check key and canonical bytes inside the
  writer transaction before current schema/head/entity/expectation checks.
  Persist the original report and key atomically. Test stale replay, changed
  requests, concurrent same-key submissions, failure without key reservation,
  and exact original reports/IDs after reopen.
- [ ] **4.4 Thin CLI and fixture parity.** Implement init/transact/pull/history/
  transactions/schema commands as needed, stdin/file input, versioned JSON output,
  structured stderr errors, and documented exit codes. Exercise the complete
  create/start/reopen/retry fixture in separate processes.

Versioned public JSON and durable canonical requests are intentionally designed
together. Storage internals may have an explicit internal encoding earlier;
they must not accidentally become an undocumented public wire contract.

## Step 5: Basic joined queries

- [ ] **5.1 AST and historical type checking.** Patterns, typed inputs/constants,
  predicates, projection, order, and limit; validate bindings and ref/entity
  unification against the view's schema.
- [ ] **5.2 Reference evaluation.** Match/filter/project/distinct/order/limit
  over replayed state. Test Ada/task joins, many-valued combinations, absent
  facts, type errors, duplicate tuples, zero limits, and total tie-breakers.
- [ ] **5.3 Public query parity.** Shared Rust/JSON conformance fixtures and CLI
  query command reproduce the design's results at A and B.
- [ ] **5.4 Bound SQL compilation.** Resolve attributes and bind values without
  string interpolation. Decide an explicit historical SQL strategy and compare
  its answers with reference evaluation before enabling it.

Replay-based historical reads require an explicit bridge to SQL querying. Choose
that bridge when implementing 5.4; current tables alone cannot serve past views.

## Step 6: Hardening and measured optimization

- [ ] **6.1 Acceptance matrix audit.** Map every design acceptance row to tests
  and remaining gaps, including all value types, schema, identity, and uniqueness.
- [ ] **6.2 Recovery and process behavior.** Multiple processes, crash/commit
  boundaries, bounded busy handling, corruption/format errors, and compatible
  reopen behavior. Validate projection rebuilding if a projection exists.
- [ ] **6.3 Backup and lineage.** SQLite-safe backups, token-preserving restore,
  documented writer retirement, and explicit unsupported divergent clones.
- [ ] **6.4 Representative measurements.** Establish workloads and performance
  needs, then choose indexes/checkpoints/projections. Compare every optimized
  path with replay, including historical schema and concurrent reads.

JMESPath, server/NIF integration, and all other deferred scope remain outside the
initial implementation. Licensing and remote destination are open repository
decisions; neither changes the local semantic implementation sequence.
