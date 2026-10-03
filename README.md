# Factlite

An embedded Rust library for a typed, SQLite-backed fact database with atomic
transactions and durable, revision-pinned reads. A thin CLI will exercise the
same library.

Factlite is being built incrementally. Typed values, schema definitions, owned
requests, and the tested private semantic evaluator are in place. SQLite storage
APIs are not implemented yet. The CLI currently supports only `--help`
and `--version`; database command interchange will follow the design.

- [Design](Factlite-Design.md): the original handoff, preserved unchanged.
- [Implementation plan](docs/plan.md): ordered items and acceptance criteria.
- [Status](docs/status.md): done, decisions, blockers, and next items.
- [Reviews](docs/reviews/): evidence recorded before each branch is merged.

The user has authorized incremental implementation and the branch, work, review,
merge workflow. The handoff's earlier requirement for separate authorization is
therefore satisfied. Recommended v1 decisions in the design are the working
contract; departures require a recorded reason and corresponding test changes.

## Development workflow

Start each bounded increment on a branch from `main`. Implement its items, run
appropriate checks, review the entire diff, address findings, and record the
review and item status before merging with a merge commit. Never claim validation
that did not run. Infrastructure-only checks establish build health, not database
correctness.

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
and documentation with warnings treated as errors. GitHub Actions uses the same
script when a remote is connected. See [testing policy](docs/testing.md) for unit,
property, integration, and scenario coverage.

The lockfile is committed. Proptest is a development dependency. SQLite and
serialization dependencies will be selected in their implementation items.

Remote hosting and licensing remain to be decided. Packages are not published.
