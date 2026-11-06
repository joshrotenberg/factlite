# Work status

## Current state

Updated 2026-11-06. This section is the summary. The entries below are the full
per-item log, oldest first, and are kept as written, so older entries describe
the state at their time. Where they disagree with this section, this section is
current.

- **Implemented:** plan steps 0 to 11, all checked in `plan.md`: the v1
  contract, plus the temporal datom index and indexed reads and commits (9.1 to
  9.5), the babashka pod (10.1), and consumer requests (11.1 to 11.5).
  Storage format 4.
- **Roadmap:**
  - Item 8: the CLI busy timeout and babashka in CI are done, while `ANALYZE`
    and per-process CLI opens remain open.
  - Item 9 (this summary) is done.
- **Tests:** 186 pass locally with babashka installed and `REQUIRE_BB=1`,
  including forty-nine 1,024-case properties and the pod's three babashka
  tests; without babashka those three skip. CI on GitHub runs the same gate on
  Linux with babashka.
- **Reviews:** entries 0001 to 0043 in `reviews/` are self-reviews. Since 0044,
  each review also records independent Claude and Codex subagent reviews, run
  with the kit subagent tooling, alongside a self-review.
- **Hosting:** private repository `github.com/joshrotenberg/factlite`. Licensed
  MIT OR Apache-2.0. Crates are unpublished.
- **Waiting on the user:** decision briefs, each in an open draft pull request
  and not yet on `main`:
  - roadmap item 3, bounded open time (PR #5);
  - item 4, lookup refs in transactions (PR #7);
  - item 5, query language (PR #8);
  - item 6, history excision (PR #9).
- **Deferred:** the JSON-RPC interface (designed, roadmap 10), the Elixir NIF,
  and pod registry publishing.

## 0.1 Inspect and initialize — done

- **Done:** Inspected the empty workspace and Git configuration. Created the
  local repository at `/workspace/factlite` with an initial `main` commit.
  Existing workspace files were left intact. No existing project instructions
  or project checkout were found.
- **Decisions:** Use a project subdirectory rather than the read-only placeholder
  `/workspace/.git`. Local merge commits will preserve each completed increment.
- **Blockers:** None for local Git. Rust was absent at inspection and is now
  installed under item 0.3. A remote destination has not been selected.
- **What's next:** Record the contract, roadmap, and workflow on
  `setup/plan-and-workflow`.

## 0.2 Record the contract and roadmap — done

- **Done:** Preserved the handoff unchanged and wrote the README, contributor
  workflow, and detailed plan with acceptance criteria. Completed a self-review,
  checked local documentation links and whitespace, and verified the original
  design is byte-identical. Review: `reviews/0001-plan-and-workflow.md`.
- **Decisions:** Adopt the design's recommended v1 semantics as the working
  contract. Use replay first and defer current projections to measured need.
  Keep each increment small enough for a complete diff review.
- **Blockers:** None.
- **What's next:** Documentation branch merged; workspace setup follows in
  items 0.3 and 0.4.

## 0.3 Scaffold the workspace — done

- **Done:** Installed Rust 1.99.0, Clippy, and rustfmt. Created the `factlite`
  library and `factlite-cli` package with a `factlite` binary, checked-in
  lockfile, workspace lints, pinned toolchain, and build instructions. CLI help
  and version work; unsupported commands exit 2 with stderr and empty stdout.
- **Decisions:** One library crate and one CLI crate; add dependencies when
  needed. Edition 2024, current stable Rust 1.99.0, and no older-compiler promise.
  All packages are unpublished. CLI usage is documented in README; binary
  rustdoc is disabled to avoid a name collision with the library documentation.
- **Blockers:** None. License and remote destination are open decisions, not
  blockers to local development.
- **What's next:** Begin typed values and identity on a new semantic-core branch.

## 0.4 Establish verification — done

- **Done:** Added executable `scripts/check.sh` and GitHub Actions configuration
  invoking the same script. Formatting, Clippy with denied warnings, the test
  runner, and library documentation passed locally. Built both packages and
  smoke-checked CLI help/version, unsupported commands, extra arguments, and
  non-UTF8 input. Checked shell syntax, staged whitespace, and package settings.
  Review: `reviews/0002-rust-workspace.md`.
- **Decisions:** Use one shared verification entry point. GitHub Actions has
  read-only contents permissions and checkout pinned to a commit. There are
  zero semantic tests at this stage; checks prove the scaffold builds only.
- **Blockers:** None locally. Remote CI is configured but cannot run until
  the repository has a remote destination.
- **What's next:** Add meaningful typed-value and identity acceptance tests with
  item 1.1, then review and merge that increment separately.

## 1.1 Typed values and identity — done

- **Done:** Six exact value types, finite-float validation/normalization,
  typed equality/hash, same-type comparisons, strict store/entity/lineage/
  snapshot tokens, revisions, and temporary labels. Eleven tests passed,
  including seven property suites with 1,024 cases each and two public
  integration tests. Format, Clippy, tests, and docs passed.
- **Decisions:** `FiniteF64` makes invalid floats unrepresentable through safe
  constructors; mixed-type ordering errors instead of coercing. Parsing IDs
  validates syntax only; item 1.2 supplies allocation membership. Token encodings
  and testing policy are recorded in `decisions.md` and `testing.md`.
- **Blockers:** None locally. No remote is needed for the workflow.
- **What's next:** Item 1.2, immutable schema and revision-aware allocation.

## 1.2 Schema and allocation state — done

- **Done:** Immutable validated attribute definitions, owned schema projections,
  revision-aware creation catalogs, staged allocations, empty entities, and
  unknown/foreign-ID validation. Eighteen tests passed, including nine properties
  at 1,024 cases each. Full checks passed.
- **Decisions:** Qualified names are exact namespace/name pairs. Identical
  redeclarations are no-ops; conflicting definitions and duplicate request
  declarations fail. Staging uses a separate value so failures leave the original
  catalog unchanged. Catalog remains private pending evaluator integration.
- **Blockers:** None locally. Durable catalog persistence is step 2 work.
- **What's next:** Item 1.3, owned transaction data and structural validation.

## 1.3 Request structure and builder — done

- **Done:** Owned typed requests/builders, declarations, operations, expectations,
  head fences, scalar metadata, and retry-key fields. Structural validation rejects
  duplicates/undeclared labels/reference metadata. Twenty-two tests passed,
  including ten 1,024-case properties and public request integration checks.
- **Decisions:** Normalize expected sets; preserve submitted operation order and
  duplicates for future retry identity. State-dependent validation waits for
  commit, after retry lookup. Builder field replacement is explicitly documented.
- **Blockers:** None locally. Serialization and actual retries remain step 4.
- **What's next:** Item 1.4, resolve and normalize batches against a reference state.

## 1.4 Pure batch normalization — done

- **Done:** Resolved temporary graphs and schema; implemented set-valued batches,
  cardinality-one replacement, duplicate/no-op collapse, exact-fact conflicts,
  deterministic effective deltas, and private reference state. Twenty-nine tests
  passed, including thirteen 1,024-case properties and four public integrations.
- **Decisions:** Known IDs must exist at pre-head; newly declared entities are
  addressed by temporary labels, even if a fabricated token guesses their future
  allocation. Compare state differences to produce effective events. Catalog's
  temporary dead-code expectation was removed after evaluator integration.
- **Blockers:** None locally. Normalization is private and is not commit;
  expectations and final-state uniqueness are the next item.
- **What's next:** Item 1.5, pre-head constraints and complete structured reports.

## 1.5 Constraints and reports — done

- **Done:** Exact pre-head conditions, head fencing, final-state uniqueness,
  complete deterministic reports, structured failure context, and reference
  scenarios. Thirty-nine tests passed, including sixteen 1,024-case properties.
  A bounded exhaustive table covers 768 batches in both operation orders and
  96 expectation cases. Full checks passed.
- **Decisions:** Evaluate conditions before staging writes, and ownership after
  complete normalization. All six value types share the same exact-set and
  ownership rules. Reports retain before/after snapshots, IDs, schema, metadata,
  and effective deltas. Large structured error context is boxed internally.
- **Blockers:** None locally. Durable atomicity, process contention, retries,
  and reopen require the upcoming SQLite implementation.
- **What's next:** Item 2.1, select the SQLite binding and define/open a versioned
  durable file with explicit durability and busy behavior.

## 2.1 Storage format and open — done

- **Done:** File-backed store creation/open, persisted random store/lineage IDs,
  full unsigned revision encoding, version/application markers, authoritative
  table layout, configured WAL/FULL/FK/busy settings, and retained SQLite sources.
  Forty-seven tests passed, including seventeen 1,024-case properties and six
  public storage integration scenarios. Full checks passed.
- **Decisions:** Rusqlite with bundled SQLite; v4 UUIDs for store/lineage entropy.
  Eight-byte big-endian revision blobs preserve unsigned SQL ordering. A mutex
  protects shared connections; separate opens use SQLite locks. Two-second busy
  timeout. Initialization is atomic and requires an empty supported file.
- **Blockers:** None locally. Full semantic-log verification is item 2.2;
  transactions and pinned views follow. Remote hosting remains intentionally local.
- **What's next:** Item 2.2, exact typed codecs, authoritative events, report
  persistence, replay, and corruption checks against the reference evaluator.

## 2.2 Authoritative log and replay — done

- **Done:** Canonical typed BLOBs, versioned internal reports/definitions,
  authoritative event/report writes, revision-bounded replay, and reopen
  consistency checks. Fifty-six tests passed, including twenty-two 1,024-case
  properties and randomized persisted revision sequences. Full checks passed.
- **Decisions:** Internal binary encoding is distinct from public JSON. Decode
  validates bounds, canonical spelling, finite floats, and exact report bytes.
  Replay cross-checks reports against schema/entity/fact events, reconstructs the
  catalog, and validates complete state. No current projection is required.
- **Blockers:** None locally. Persistence helpers are private; public commits,
  rollback outcomes, and concurrency are items 2.3–2.4.
- **What's next:** Item 2.3, one serialized SQLite write transaction per commit,
  request validation/evaluation, rollback classification, and public API tests.

## 2.3 Serialized commit — done

- **Done:** Public `Store::commit`, serialized head capture/evaluation/report/event
  persistence, explicit rollback, fresh no-change revisions, and commit outcome
  evidence. Sixty tests passed, including twenty-two 1,024-case properties and
  four new public commit integration scenarios. Full checks passed.
- **Decisions:** Keep the transaction guard until COMMIT errors are inspected;
  drop-time rollback cannot provide explicit outcome evidence. Failed pre-write
  validation and successful rollback report RolledBack; unconfirmed COMMIT
  completion reports Unknown. Reject keyed requests until exact retries exist.
- **Blockers:** None locally. Injected SQL/COMMIT failures and competing writers
  are next; actual power-loss/crash recovery remains later hardening.
- **What's next:** Item 2.4, faults at every persistence stage, commit-error paths,
  bounded busy behavior, and competing conditional updates/connections.

## 2.4 Failure and concurrency checks — done

- **Done:** Eight SQL write fault locations, deferred COMMIT failure, ended-
  transaction outcome classification, competing conditions, twelve concurrent
  no-change writers, unrelated-change fencing, and bounded commit Busy errors.
  Sixty-seven tests passed; twenty-two 1,024-case properties remain in the gate.
- **Decisions:** Test hooks/temporary triggers stay test-only. Every injected
  write fault preserves old head/report/metadata and leaves no schema/entities/
  facts. COMMIT errors are classified conservatively. Separate connections
  serialize revisions and only one competing condition succeeds.
- **Blockers:** None locally. Physical power-loss/process-kill/IO recovery and
  multi-process CLI behavior are later hardening; these tests do not claim them.
- **What's next:** Item 3.1, shareable pinned views and full snapshot validation.

## 3.1 Pinned Db ownership and tokens — done

- **Done:** Shareable pinned views, historical schema, full identity/bounds
  validation, and durable use after dropping Store or closing/reopening all
  handles. Seventy tests passed, including twenty-three 1,024-case properties.
  Full checks passed; review: `reviews/0012-pinned-db-views.md`.
- **Decisions:** Each read owns one short SQLite transaction and returns owned
  data. Db retains a shared durable handle, never a long-lived transaction or
  implicit refresh. Invalid/foreign/future snapshots fail without clamping.
- **Blockers:** None locally. Remote CI has not run.
- **What's next:** Item 3.2, facts, flat pull, bounded history and transactions.

## 3.2 Revision-bounded reads — done

- **Done:** Owned facts, flat projections, effective history with original event
  ordinals, and complete transaction records including no-change commits and
  metadata. Seventy-four tests passed, including twenty-four 1,024-case properties.
  Full checks passed; review: `reviews/0013-revision-bounded-reads.md`.
- **Decisions:** Validate filters against historical schema, preserve exact typed
  values, sort facts and many values deterministically. Empty attribute lists
  project no fields; unknown/foreign entities match no facts or pull None.
  Reject lower bounds above the basis. All dependent SQL stays in one transaction.
- **Blockers:** None locally. Query execution, interchange, and CLI are pending.
- **What's next:** Item 3.3, the supplied Ada/task fixture through public durable
  APIs, including condition failures, unrelated changes, and head fences.

## 3.3 A/B/reopen acceptance fixture — done

- **Done:** The supplied graph, conditional todo-to-doing replacement, explicit
  reference traversal, identical A reads after B and reopen, full reports,
  metadata, and bounded status history. Condition failures and head fences are
  verified through public durable APIs. Seventy-six tests and full checks passed.
- **Decisions:** Close every handle before reopen. Keep query and exact-retry
  assertions explicitly pending their APIs; no keyed fixture is accepted early.
  Review: `reviews/0014-ada-task-reopen-fixture.md`.
- **Blockers:** None locally. CLI/process and retry parity belong to step 4.
- **What's next:** Item 3.4, force writer commits between dependent reads and
  verify the operation retains one SQLite snapshot.

## 3.4 Reader/writer isolation — done

- **Done:** Deterministically forced B between A's head check and log replay for
  schema, facts, pull, history, and transactions. A separate boundary test verifies
  SQL head/log consistency while a WAL writer commits. Seventy-eight tests and
  full checks passed; review: `reviews/0015-reader-writer-isolation.md`.
- **Decisions:** Test-only SQLite authorizer callbacks trigger the precise gap;
  no scheduling sleeps or production hook APIs. Fresh views see B after the
  operation; the old view stays A. No current projection exists to race yet.
- **Blockers:** None locally. Remote CI and physical crash recovery remain later.
- **What's next:** Item 4.1, strict versioned JSON request and snapshot parsing.

## 4.1 Versioned wire decoding — done

- **Done:** Strict version-one request/snapshot/value decoders and encoders,
  recursive duplicate-key detection, field/tag checks, explicit defaults, typed
  precision, normalized floats/bytes, and Rust/JSON durable commit parity.
  Eighty-five tests passed, including twenty-seven 1,024-case properties; full
  checks passed. Review: `reviews/0016-strict-versioned-json.md`.
- **Decisions:** Keep interchange in a separate library module. Use validated
  constructors rather than deserializing private identity/value fields. Accept
  alternate decimal spellings and standard base64 padding, emit canonical forms;
  explicit null is invalid. Preserve operation/declaration arrays.
- **Blockers:** None locally. Decoding a keyed request does not enable retries;
  keyed commit remains rejected until lookup/persistence land together.
- **What's next:** Item 4.2, versioned canonical typed request bytes and equality.

## 4.2 Canonical request representation — done

- **Done:** Read-only canonical request evidence, frozen version-one binary
  encoding, full typed payload, raw normalized float bits, and golden boundaries.
  Ninety-one tests passed, including thirty 1,024-case properties; full checks
  passed. Review: `reviews/0017-canonical-typed-requests.md`.
- **Decisions:** Exclude key, preserve declaration/operation/expectation-entry
  arrays and temporary labels, normalize expected sets and metadata key order.
  Compare complete bytes, never only a digest. Canonical encoding is independent
  of JSON/report codecs and does not depend on head or resolved IDs.
- **Blockers:** None locally. Canonical evidence is not yet persisted by commit.
- **What's next:** Item 4.3, atomic key lookup before state validation, durable
  canonical evidence, original-report replay, and concurrency/failure scenarios.

## 4.3 Atomic retry lookup — done

- **Done:** Serialized key lookup before state checks, atomic key/canonical/report
  persistence, exact original reports after later commits/reopen, bounded retained
  canonical decoding, and request/report coherence checks during replay. All 101
  tests passed, including thirty-three 1,024-case properties; full checks passed.
- **Decisions:** Complete byte equality determines replay/conflict. Same-key races
  allocate once; different requests have one winner. Failed SQL/COMMIT requests
  leave keys reusable. No key expiry/pruning. Existing unkeyed files still work.
  Review: `reviews/0018-atomic-exact-retries.md`.
- **Blockers:** None locally. CLI/process retry parity is next.
- **What's next:** Item 4.4, functional commands, versioned stdout/structured
  stderr, and complete create/start/reopen/retry scenarios using assert_cmd.

## 4.4 Thin CLI and fixture parity — done

- **Done:** Seven database commands, stdin/files, portable/compact pinned tokens,
  lossless JSON responses, structured stderr, and documented exit taxonomy.
  Six assert_cmd scenarios include the complete design fixture, exact full-report
  Rust parity, all types, every stable error code, and process reopen. All 107
  tests passed, including thirty-three 1,024-case properties; full checks passed.
- **Decisions:** CLI initialization is explicit; parse/validate inputs before
  opening. Native file paths survive non-UTF8 Unix names. Every database command
  emits one JSON result; help/version remain human-readable. Executed the documented
  tutorial successfully. Review: `reviews/0019-cli-design-fixture-parity.md`.
- **Blockers:** None locally. Sandbox socket access was needed by assert_cmd's
  timeout helper and supplied for the successful gate; remote CI has not run.
- **What's next:** Item 5.1, bounded typed query AST, bindings/parameters, historical
  schema checking, selected ordering, and rejected incompatible uses.

## 5.1 AST and historical type checking — done

- **Done:** Typed pattern/parameter/variable/constant AST, comparisons, selected
  columns/order, unsigned limits, and validation against pinned historical schema.
  All 112 tests passed, including thirty-five 1,024-case properties; full checks
  passed. Review: `reviews/0020-query-ast-validation.md`.
- **Decisions:** Entity positions unify only with Ref. Exact equality preserves
  different types as unequal; ranges require identical domains. Every declared
  parameter is required, including unused declarations. Validate before zero-limit
  short-circuit. Query constants need type validity, not allocation membership.
- **Blockers:** None locally. Validation deliberately performs no fact matching.
- **What's next:** Item 5.2, conjunctive reference evaluation, distinct projection,
  ordered limits, Ada/task joins, many-valued combinations, and pinned recovery.

## 5.2 Reference evaluation — done

- **Done:** Public Db::query, conjunctive joins including entity/reference
  unification, exact comparisons, projection/distinct/order/limit, and immutable
  historical results. All 117 tests passed, including thirty-six 1,024-case
  properties; full checks passed. Review: `reviews/0021-reference-query-evaluation.md`.
- **Decisions:** Evaluate over replayed facts as the correctness oracle. Empty
  conjunctions have an identity row; distinct empty projection has at most one
  tuple. Typed entities return Ref. No implicit tie ordering. Query uses the same
  read transaction and now passes the forced writer interleaving too.
- **Blockers:** None locally. Public JSON/CLI and SQL query execution are next.
- **What's next:** Item 5.3, shared query envelopes and subprocess A/B parity.

## 5.3 Public query parity — done

- **Done:** Strict versioned query/params decoding, lossless encoding and named
  typed result rows, query CLI, and executable design fixture. All 122 tests
  passed, including thirty-eight 1,024-case properties; full checks passed.
  Review: `reviews/0022-query-wire-cli-parity.md`.
- **Decisions:** Share historical Rust validation for JSON/CLI. Require match and
  select; default inputs/where/order/params empty. Use left/op/right predicates.
  Reject malformed/duplicate/unknown fields and simultaneous stdin inputs.
- **Blockers:** None locally. No remote CI or independent review has run.
- **What's next:** Item 5.4, bound SQL over an explicit historical relation with
  reference parity before making SQL the public execution path.

## 5.4 Bound SQL compilation — done

- **Done:** Bound joins over a pinned replay-derived TEMP relation, exact typed
  predicates, distinct/order/limit and owned decoding. All 126 tests passed,
  including thirty-nine 1,024-case properties; full checks passed.
  Review: `reviews/0023-bound-historical-sql.md`.
- **Decisions:** Numeric attribute IDs and generated aliases; bind all caller
  values. Keep a reference fallback for large ASTs beyond conservative SQL
  budgets. No durable current projection. Rebuild historical relation per query.
- **Blockers:** None locally. No performance target or optimization claim.
- **What's next:** Item 6.1, audit each acceptance row and identify exact remaining
  process-recovery/backup evidence before hardening and measurement.

## 6.1 Acceptance matrix audit — done

- **Done:** Mapped every original acceptance row to concrete test files/names in
  `acceptance.md`, verified all 35 evidence links, and added durable all-six-type
  uniqueness transfer/failure/retry/reopen coverage. All 127 tests passed,
  including thirty-nine 1,024-case properties; full checks passed.
  Review: `reviews/0024-acceptance-audit.md`.
- **Decisions:** Existing semantic acceptance is covered; process-crash/lost-response
  evidence follows in 6.2 and backup/read-only/faithful-restore evidence in 6.3.
  Conditional projection tests remain conditional: no durable projection exists.
- **Blockers:** None locally. The audit marks unfinished acceptance honestly.
- **What's next:** Item 6.2, separate-process contention and termination around
  COMMIT, preserving complete report/key/metadata recovery.

## 6.2 Recovery and process behavior — done

- **Done:** Deterministic pre/post COMMIT process termination with verified WAL
  spill, complete old/new state recovery and exact lost-response retries. Eight
  CLI processes prove same-key allocation once and one conditioned-update winner.
  All 129 tests passed, including thirty-nine 1,024-case properties; full checks
  passed. Review: `reviews/0025-process-recovery.md`.
- **Decisions:** Test-only child/hook controls, bounded pipe handshake, abrupt kill
  and cleanup; no scheduling sleeps or physical power-loss claim. Existing busy,
  format and corruption coverage remains. No projection requires rebuilding.
- **Blockers:** None locally.
- **What's next:** Item 6.3, safe consistent backup publication, read-only access,
  token-preserving faithful restore and explicit lineage operational rules.

## 6.3 Backup and lineage — done

- **Done:** Fixed-snapshot SQLite backup, validated standalone no-replace
  publication, read-only historical constructors, CLI backup and read-only reads.
  Forced concurrent writer, failure/race cleanup, faithful retired-writer restore
  and all-type backup history/query property pass. All 136 tests passed, including
  forty 1,024-case properties; full checks passed.
  Review: `reviews/0026-backup-lineage.md`.
- **Decisions:** Preserve every token/identity/report/key; capture the copied head.
  Caller must retire the prior writer and preserve identical complete committed
  history before resuming lineage. No divergent clones, incomplete restore,
  automatic clone detection or physical power-loss guarantee. See `backup.md`.
- **Blockers:** None locally. Remote CI and independent review have not run.
- **What's next:** Item 6.4, reproducible current/historical/read/write workloads,
  latency measurements and only justified index/checkpoint/projection choices.

## 6.4 Representative measurements — done

- **Done:** Release workload runner with independent expected typed answers;
  four graph/history configurations on workspace overlay and tmpfs, fifteen
  samples per operation, raw median/p95 outputs and reproducible commands.
  Custom boundary/small runs and invalid inputs/cleanup verified. All 136 tests
  passed, including forty 1,024-case properties; full checks passed.
  Review: `reviews/0027-workload-measurements.md`.
- **Decisions:** Replay/validation is the dominant measured cost: 1,000 tasks at
  revision 50 took about 182ms per head query versus 24ms at revision 1. Keep
  existing indexes and correctness-first replay; choose cache/checkpoint budgets
  after a real workload/latency target is established. See `measurements.md`.
- **Blockers:** None locally. Production workload/SLA, remote and license remain
  open decisions; no remote CI or independent review has run.
- **What's next:** Planned local v1 sequence is complete. Review first-use
  behavior/performance needs; package Git history, source, lockfile and reviews
  into a tarball when requested for the next remote-connected session.

## 7.1 First-use API pass — done

- **Done:** Added `docs/rust.md`, an executable task workflow and a minimal
  rustdoc example. The walkthrough covers forward references, pinned traversal,
  conditional updates, joined queries, stale conditions/fences, exact retries,
  no-change commits, portable tokens, backup, read-only access and reopen.
  All 137 tests, the walkthrough and the full gate passed.
  Self-review: `reviews/0028-rust-first-use.md`.
- **Decisions:** Keep explicit typed constructors and current APIs; no concrete
  semantic/API defect emerged. Run the asserted walkthrough in the standard
  gate and document application error/recovery choices.
- **Blockers:** None locally; remote and license are still unselected.
- **What's next:** Combine operations in shrinking stateful traces against an
  independent domain oracle (7.2), then profile replay (7.3).

## 7.2 Combined stateful testing — done

- **Done:** Added an independent domain oracle and a deterministic mixed
  scenario plus 1,024 shrinking operation traces. Sixteen command kinds mix
  schema, references/cycles, unique transfer, sets, conditions/fences, failures,
  exact/changed retries, historical reads and real close/reopen. Expected IDs,
  complete reports and event history are calculated before submission. All
  139 tests and full checks passed. Review: `reviews/0029-mixed-stateful-traces.md`.
- **Decisions:** Use small reproducible traces and a distinct domain model;
  keep broader typed, concurrent, fault and recovery suites. Each failure is
  followed by a clean commit under its key to test nonreservation. No production
  defect emerged from these traces.
- **Blockers:** None locally.
- **What's next:** Measure replay stages and implement one justified improvement
  with optimized/reference parity and the complete recovery/isolation gate (7.3).

## 7.3 Replay profiling and verified state reuse — done

- **Done:** Added release stage profiling and fresh before/after workloads.
  Keyed replay now reuses its evaluated state after exact full-report equality;
  event/canonical checks and unkeyed report application remain. Typed randomized
  parity, future allocation and joint report/event tampering checks passed.
  The full gate passed 142 tests, including forty-two 1,024-case properties,
  the Rust walkthrough, and all fault/isolation/recovery/backup/CLI checks. The
  manual profiler passed separately. Review: `reviews/0030-verified-replay-state-reuse.md`.
- **Decisions:** Remove demonstrated duplicate work without changing storage or
  SQL snapshot boundaries. At 1,000 tasks/50 revisions, head-query median changed
  from 180.394 to 85.531ms; at 100/250, 89.986 to 43.527ms. Keep the original path
  in test builds. These are warm-container observations, not an SLA; caches and
  checkpoints still require workload/budget decisions. See `measurements.md`.
- **Blockers:** None locally. Remote hosting and license remain open; reviews
  are self-reviews and remote CI has not run.
- **What's next:** All authorized items are complete locally. Prepare the Git,
  source, lockfile and review tarball when requested for a remote-connected
  session. Select remote/license and a production workload before further
  performance scope or publication.

## 8.1 Prepare and verify handoff — done

- **Done:** Added `docs/handoff.md` with restore, verification, remote publishing
  and resume instructions. Exported a clean checkout from a complete Git bundle,
  retaining all branch names and reachable history. Independently restored it
  and compared commit/tree, refs, commit count and every tracked file. Git
  integrity, formatting, locked workspace metadata, the Rust workflow and CLI
  version all passed in the restored checkout. Review: `reviews/0031-handoff-preparation.md`.
- **Decisions:** Include `.git`, source, lockfile, guides, reviews and benchmark
  evidence. Exclude build outputs, toolchains, databases, credentials and machine
  Git configuration. Deliver a separate manifest and SHA-256 checksum; regenerate
  the archive for the exact final published main commit.
- **Blockers:** None for packaging. GitHub CLI authentication works for
  `joshrotenberg`; publication and remote verification remain item 8.2.
- **What's next:** Publish to the selected remote and compare all branch refs
  plus a fresh clone before declaring the remote handoff complete.

## 9.1 Temporal datom index: done

- **Done:** Format 2 adds a `datoms` interval table, written inside each commit
  transaction. Reads and commit evaluation load state at R from bounded schema/
  entity events and visible intervals. Every open runs full replay, compares
  stored intervals with intervals derived from fact events, and cross-checks
  indexed head state against replay. Writable opens upgrade format-1 files
  atomically. All 147 tests passed, including forty-three 1,024-case properties;
  full checks passed. Review: `reviews/0032-temporal-datom-index.md`.
- **Decisions:** The log stays authoritative and the index is derived and
  fail-closed. Integrity is established at open, not per read. Read-only opens
  of format-1 files fail `UnsupportedFormat`. Columns reuse fact-event encodings
  (text identities). Queries keep the TEMP relation for now. See `decisions.md`.
- **Measured:** On an Apple M4 Pro at 1,000 tasks/50 revisions, head-query median
  fell from 41.088 to 4.922ms and no-op commit from 45.124 to 2.901ms. Read-only
  open rose 5–28%, and per-process CLI commands still pay full replay. See
  `measurements.md`.
- **Also:** The CLI non-UTF-8 path test now runs its file round trip only where
  the filesystem accepts such names. APFS rejects them, so it failed on macOS.
- **Blockers:** None locally. No git remote is configured; license is unselected.
- **What's next:** Item 9.2, revision-bounded SQL for facts/pull/query directly
  over intervals. Open-time cost (full replay per open) is a separate decision.
  Options include an explicit verify command, or verified checkpoints that bound
  replay at open.

## 10.1 Babashka pod: done

- **Done:** Added `factlite-pod` (binary `pod-factlite`, namespace
  `pod.joshrotenberg.factlite`) on `babashka-pod` 0.1.1. It has open/close,
  head, transact, schema, pull, facts, history, transactions and q, with `:at`
  snapshots or revisions. All 158 tests passed, including forty-four 1,024-case
  properties and 59 babashka checks on both transports; full checks passed.
  The README example ran with the documented output.
  Review: `reviews/0033-babashka-pod.md`.
- **Decisions:** Clojure data is translated into version-1 wire JSON, so the
  library's strict decoders validate it. There is one Clojure type per value
  type and no coercion. Errors are ex-info with stable code, context and commit
  outcome. Pod and CLI requests share canonical retry identity. See
  `decisions.md` and `crates/factlite-pod/README.md`.
- **Concurrency:** Four pod processes (8 x 60 in a manual heavier run) committed
  to one file with no lost, merged or duplicated writes. Conditional increments
  serialized with 183 rereads and retries in the heavy run.
- **Size:** Release `pod-factlite` on macOS arm64 is 3.4 MB stripped and 1.60 MB
  zipped.
- **Blockers:** None locally. No remote or CI has run the babashka tests; they
  skip without `bb` unless `REQUIRE_BB` is set.
- **What's next:** Registry packaging/release once a remote exists. Read-only
  opens through the pod if a consumer needs them. Item 9.2.

## 9.3 Open without the write lock: done

- **Done:** `Store::open` validates an existing store in a deferred read
  transaction and takes the write lock only to initialize or upgrade. All 160
  tests passed, including forty-four 1,024-case properties; full checks passed.
  Review: `reviews/0034-open-without-write-lock.md`.
- **Found by:** the clajent session's pod trial. Opens of a 2,803-revision store
  held the write lock for 7.5 s, and concurrent writers saw 2.2 s `Busy` failures.
  On a copy of that store after the fix, one pod process committed 200 times
  while another opened it three times: zero `Busy`, worst commit 21 ms.
- **Decisions:** See `decisions.md`. Validation still covers a full snapshot at
  open; later commits are validated by the next open.
- **Blockers:** None locally.
- **What's next:** Open time itself grows with revisions times live state (7.5 s
  at 2,803 revisions in that store). Profiling shows per-revision whole-state
  work in replay validation. Choosing incremental validation, checkpoints or an
  explicit verify step needs a user decision.

## 9.4 Incremental replay validation: done

- **Done:** Transitions run in place and check only the facts they touch, using
  indexes maintained across replay. All 162 tests passed, including forty-five
  1,024-case properties; full checks passed.
  Review: `reviews/0035-incremental-replay-validation.md`.
- **Decisions:** Chosen by the user over checkpoints or an explicit verify step.
  Open still validates every invariant at every revision; the induction argument
  and retained full-state reference are in `decisions.md`.
- **Measured:** Open on a copy of the clajent store fell from about 7.5 s to
  0.14 s. At 1,000 tasks, open is 46 ms at 2,000 revisions (was 1.29 s) and
  138 ms at 8,000. See `measurements.md`.
- **Blockers:** None locally.
- **What's next:** Commits and reads load the full state at head, so they are
  linear in live state (163 ms commits at 120,000 facts). Item 9.2 addresses
  reads; commits would need state loaded only for the entities and attributes a
  request touches.

## 9.2 Direct indexed reads: done

- **Done:** Reads and queries use revision-bounded SQL over `datoms` instead of
  loading the state at R. Format 3 adds an attribute/value index, and writable
  opens upgrade formats 1 and 2. All 165 tests passed, including forty-six
  1,024-case properties; full checks passed.
  Review: `reviews/0036-direct-indexed-reads.md`.
- **Measured:** At 120,001 facts, the joined head query fell from 395 ms to
  0.13 ms, and it is about 0.12 ms at every measured size. See `measurements.md`.
- **Decisions:** See `decisions.md`. No TEMP relation; large ASTs keep the
  reference fallback.
- **Blockers:** None locally.
- **What's next:** Item 9.5, commits loading only the state a request touches.

## 9.5 Commits load only touched state: done

- **Done:** Commits evaluate against the state a request can observe instead
  of the full state at head. All 167 tests passed, including forty-seven
  1,024-case properties; full checks passed.
  Review: `reviews/0037-touched-commit-state.md`.
- **Measured:** No-op commit at 120,001 facts fell from 173.5 ms to 0.09 ms.
  Through the pod on the clajent store copy, a 13-fact create took 0.52 ms
  (p50). See `measurements.md`.
- **Decisions:** See `decisions.md`.
- **Blockers:** None locally.
- **What's next:** Open replays the whole log and is linear in revisions plus
  live state (870 ms at 120,001 facts). Verified checkpoints would bound it if a
  consumer needs faster opens. The smaller clajent requests remain open: lookup
  refs, aggregates, keyword/inst types, a busy-timeout option, and read-only
  open in the pod.

## 11.1 Open options and read-only pod connections: done

- **Done:** `StoreOptions` with a validated busy timeout, `Db::at_head`/`Db::at`
  for explicit refresh of read-only views, and pod `open` options
  `:read-only` and `:busy-timeout-ms`. All 169 tests passed; full checks
  passed. Review: `reviews/0038-open-options.md`.
- **Decisions:** See `decisions.md`.
- **Blockers:** None locally.
- **What's next:** 11.2 lookup refs, then 11.3 indexed filtered history.

## 11.2 Lookup refs for reads: done

- **Done:** `Db::lookup`, CLI `lookup`, and pod lookup refs in `pull`,
  filters and a `lookup` var. All 172 tests passed; full checks passed.
  Review: `reviews/0039-lookup-refs.md`.
- **Decisions:** Reads only; transactions keep `{:id}`/`{:temp}`. See
  `decisions.md`.
- **Blockers:** None locally.
- **What's next:** 11.3 indexed filtered history.

## 11.3 Indexed filtered history: done

- **Done:** History filtered by entity or attribute reads datom intervals and
  per-revision events instead of every report. All 173 tests passed, including
  forty-eight 1,024-case properties; full checks passed.
  Review: `reviews/0040-indexed-history.md`.
- **Measured:** Entity and attribute history on the clajent store copy fell
  from 16.3 ms to 0.12 ms. See `measurements.md`.
- **Blockers:** None locally.
- **What's next:** Aggregates and new value types (keyword, inst) were deferred
  in the original design and need a user decision before any work.

## 11.4 Aggregates: done

- **Done:** count, count-distinct, sum, min and max with grouping by selected
  variables and an explicit `with` list, in Rust, JSON, CLI and the pod. All
  179 tests passed, including forty-nine 1,024-case properties; full checks
  passed. Review: `reviews/0041-aggregates.md`.
- **Decisions:** Distinct-tuple semantics, exact-or-error arithmetic, and no
  rows for no matches. See `decisions.md`.
- **Measured:** Cost per recipe over 1,401 runs on the clajent store copy took
  1.85 ms through the pod, against 4.7 ms to fetch and reduce in babashka.
- **Blockers:** None locally.
- **What's next:** New value types (keyword, inst) remain undecided.

## 11.5 Keyword and inst value types: done

- **Done:** Keyword and inst values in the library, storage (format 4), wire,
  canonical requests, queries, CLI and pod. All 181 tests passed, including
  forty-nine 1,024-case properties, now over eight types; full checks passed.
  Review: `reviews/0042-keyword-inst-types.md`.
- **Upgrade evidence:** A backup copy of the clajent store (format 3, 24,452
  events) upgraded on first open in 247 ms. History at its last pre-upgrade
  revision is byte-identical to the previous CLI's output, and a keyword fact
  then committed.
- **Decisions:** See `decisions.md`.
- **Blockers:** None locally. No remote is configured.
- **What's next:** All clajent requests are addressed except transaction
  lookup refs, which need a canonical-identity decision.

## 8.2 Publish and verify: done

- **Done:** The user created the private repository
  `github.com/joshrotenberg/factlite` and pushed `main`. All 45 branches were
  then pushed. Every remote branch ref equals the local ref. A fresh mirror
  clone passes `git fsck --full` with the same 89 reachable commits. A fresh
  working clone checks out `main` at `250bc6b` with tree `3f1e186` and
  byte-identical tracked files, and the asserted Rust workflow example runs
  from it. Review: `reviews/0043-publish-verified.md`.
- **Remote CI:** The first run (`main`, `250bc6b`, ubuntu-latest, Rust 1.99.0)
  passed `scripts/check.sh`: 181 tests, 1 ignored. babashka is not installed on
  the runner, so the three pod babashka tests skip there.
- **Branch runs:** Pushing the historical branches started 32 runs although the
  workflow was disabled during the push: GitHub processed the push events after
  it was re-enabled. 27 were cancelled and 5 completed successfully; none
  failed. The push trigger is now limited to `main`, with pull requests
  covering branches.
- **Archive:** `factlite-handoff.tar.gz` (757,342 bytes), its `.sha256` and
  `factlite-handoff-manifest.json` were built from the remote for `250bc6b` and
  delivered to the user. They are not stored in the repository.
- **Decisions:** No license is selected; the repository is private.
- **What's next:** See `roadmap.md`. Future items use draft pull requests.

## Concurrent first-open fix: done

- **Done:** `Store::open` retries the switch to WAL within its busy timeout
  when several connections open a new file at once, and labels its SQLite
  steps in errors. Found by CI on PR #5. Review:
  `reviews/0045-concurrent-first-open.md`.
- **Evidence:** before the fix, the stress test failed 7 of 10 runs; after it,
  none. New deterministic tests cover a blocked switch timing out within budget
  and succeeding after release. 183 tests pass.
- **What's next:** roadmap item 4 (lookup refs in transactions) needs a
  decision brief; item 3's brief (PR #5) awaits the user.

## Roadmap 8: CLI busy timeout and babashka in CI: done

- **Done:** `--busy-timeout-ms` on `init` and every database command, and CI
  that installs a checksum-pinned babashka and requires the pod's babashka
  tests. 184 tests pass with `REQUIRE_BB=1`.
  Review: `reviews/0046-cli-timeout-ci-babashka.md`.
- **Left open:** `ANALYZE` (no measured need), and per-process CLI opens, which
  wait on the item 3 decision.
- **What's next:** item 9 (documentation) needs a decision brief; items 3, 4, 5
  and 6 await decisions in PRs #5, #7, #8 and #9.

## Roadmap 9: current-state documentation: done

- **Done:** a current-state section at the top of this file, and a rewritten
  README overview and `handoff.md`. Review: `reviews/0047-current-state-docs.md`.
- **Decisions:** this log stays as written, as `AGENTS.md` requires; the
  summary replaces only the drifting opening paragraph.
- **Blockers:** none.
- **What's next:** every remaining roadmap item is done, deferred, or waiting
  on a decision brief (PRs #5, #7, #8, #9).

## Roadmap 8: documentation link check: done

- **Done:** an integration test that every relative Markdown link and image
  resolves, parsed with pulldown-cmark. 186 tests pass with `REQUIRE_BB=1`.
  Review: `reviews/0048-doc-link-check.md`.
- **Decisions:** a test-only dependency on pulldown-cmark rather than
  hand-written parsing, after both reviews found the hand-written parser unsafe.
- **What's next:** roadmap item 8 still lists `ANALYZE` (no measured need) and
  per-process CLI opens (waiting on item 3). Everything else is done, deferred
  or waiting on decision briefs #5, #7, #8 and #9.

## Environment notes

The local toolchain lives outside Git in `/workspace/.factlite-tools`. For this
environment, source `/workspace/.factlite-tools/env.sh` before running Cargo.
Standard development machines can use their usual rustup installation.
The managed sandbox must permit local sockets when running assert_cmd timeouts;
the successful check invocation granted network/socket access while Cargo stayed
offline. The database and CLI require no outbound network access.

## Open repository decisions

- Remote hosting destination: private `github.com/joshrotenberg/factlite`.
- License: MIT OR Apache-2.0 (chosen 2026-11-05). Crates remain unpublished.
