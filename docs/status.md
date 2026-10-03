# Work status

Step 1 and items 2.1–2.3 are complete. Public atomic commits persist reports and
retained history. Next is item 2.4, injected-failure and concurrent-writer evidence.
Pinned reads and database CLI commands remain pending. Reviews are self-reviews;
remote CI has not run.

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

## Environment notes

The local toolchain lives outside Git in `/workspace/.factlite-tools`. For this
environment, source `/workspace/.factlite-tools/env.sh` before running Cargo.
Standard development machines can use their usual rustup installation.

## Open repository decisions

- Remote hosting destination: none selected; branch/review/merge is local.
- License: none selected; publishing is disabled.
