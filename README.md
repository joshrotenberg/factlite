# Factlite

An embedded Rust library for a typed, SQLite-backed fact database with atomic
transactions and durable, revision-pinned reads. A thin CLI and a babashka pod
expose the same library.

Current state (format 4): see [status](docs/status.md) for what is implemented and
[roadmap](docs/roadmap.md) for what is not.

- **Data model:** typed facts with eight value types (bool, i64, f64, string,
  bytes, entity reference, keyword, inst). Typed equality never coerces.
- **Transactions:**
  - immutable schema, with cardinality-one and cardinality-many attributes and
    unique values;
  - atomic declarative batches, validated against their final state;
  - exact conditional updates, and fencing on an expected head;
  - exact idempotent retries.
- **Reads:** `Store::db()` and `db_at()` return views pinned to a revision,
  which stay stable across reopen and backups. Facts, pull, lookups by unique
  value, entity- or attribute-filtered history and joined queries (with `not`
  and `not-join`) run as
  revision-bounded SQL over a derived datom interval index. Schema reads
  schema events. Transaction enumeration and unfiltered history read the
  retained reports. Aggregates (count, count-distinct, sum, avg, min, max,
  with HAVING) are computed in Rust over the distinct tuples
  that SQL returns, and very large queries fall back to the reference
  evaluator.
- **Commits:** each commit loads only the state its request can touch.
- **Integrity:** every open replays and validates the retained log
  incrementally and checks the index against it. The log stays authoritative.
- **Interfaces:** the Rust library is the core. A CLI and a babashka pod
  (`crates/factlite-pod`) are thin adapters over the same strict, versioned JSON
  wire contract. So is `factlite-rpc` (`crates/factlite-rpc`), a JSON-RPC 2.0
  server over stdio for a client in any language.
- **Operations:** WAL with FULL synchronization and a configurable busy
  timeout. Opening an existing current-format store validates it without
  blocking writers; initialization and format upgrades take the write lock.
  SQLite-safe standalone backups and crash recovery are tested.
- **Permanence:** committed values, metadata and retry records are kept for the
  life of the store and copied into later backups. Nothing deletes them. Keep
  data that may have to be deleted outside the store, or encrypt it under a key
  the application can destroy; see [permanence](docs/permanence.md).

- [Design](Factlite-Design.md): the original handoff, preserved unchanged.
- [Implementation plan](docs/plan.md): ordered items and acceptance criteria.
- [Status](docs/status.md): done, decisions, blockers, and next items.
- [Roadmap](docs/roadmap.md): candidate work not yet authorized, and the decision each needs.
- [JSON-RPC server](docs/rpc.md): `factlite-rpc` over stdio: methods, handles, rules and errors.
- [JSON-RPC design](docs/design/json-rpc.md): why the process-boundary interface has the shape it has.
- [Acceptance audit](docs/acceptance.md): tests mapped to the supplied contract.
- [Reviews](docs/reviews/): evidence recorded before each branch is merged.
- [Babashka pod](crates/factlite-pod/README.md): Clojure API, value mapping and errors.
- [Measurements](docs/measurements.md): reproducible workloads, replay and index costs.
- [Backups](docs/backup.md): SQLite-safe copies, read-only access, and lineage rules.
- [Permanence](docs/permanence.md): retained history, sensitive data, encryption limits and physical deletion.
- [Comparison](docs/comparison.md): features compared with Datomic, XTDB, Datahike, Datalevin, DataScript, Mentat, CozoDB and SQLite.
- [Agent guide](docs/agents.md): what agents must never write, a store per scope, and conventions for provenance, keys, snapshots and handoff.
- [Work queues](docs/queues.md): a tested queue pattern with leases and takeover, its guarantees and measured costs.
- [Agent-native exploration](docs/design/agent-native.md): Datalevin's MCP interface, and Factlite for agent context, history, recall and work queues. Undecided.
- [CLI](docs/cli.md): commands, output/exit contracts, and a runnable Ada/task example.
- [Rust guide](docs/rust.md): an executable application workflow and API choices.
- [Handoff](docs/handoff.md): clone, verify and resume work.

Recommended v1 decisions in the design are the working contract; departures
require a recorded reason and corresponding test changes, recorded in
[decisions](docs/decisions.md).

## Development workflow

Start each bounded increment on a branch from `main`, with a draft pull request
whose description is the plan. Implement it, run `scripts/check.sh`, and record
the review in `docs/reviews/`. Implementation PRs are reviewed by independent
Claude and Codex subagents, and findings are checked against the code before
they are fixed or rejected. Merge with a merge commit once CI passes. Never
claim validation that did not run.

## Build and check

Install [rustup](https://rustup.rs/). `rust-toolchain.toml` pins Rust 1.99.0 with
Clippy and rustfmt. The workspace uses Rust edition 2024 and does not currently
promise compatibility with older compilers.

```sh
./scripts/check.sh
cargo run --locked -p factlite-cli -- --version
cargo run --locked -p factlite-cli -- --help
```

The check script runs formatting, Clippy with warnings treated as errors, tests,
the asserted Rust workflow example,
and documentation with warnings treated as errors. GitHub Actions runs the same
script on pull requests and pushes to `main`, with babashka installed so the
pod's babashka tests run. See [testing policy](docs/testing.md) for unit,
property, integration, and scenario coverage.

The lockfile is committed. Storage uses rusqlite with bundled SQLite and UUIDs
for durable store/lineage identity. Proptest and tempfile support test coverage.

Storage configures WAL, FULL synchronization, foreign keys, and a busy timeout
(two seconds unless set through `StoreOptions` or `--busy-timeout-ms`). A live store may have SQLite sidecar files; `Store::backup` uses SQLite
to produce a validated standalone copy. See [storage decisions](docs/decisions.md).

The repository is hosted publicly on GitHub. Packages are not published.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
