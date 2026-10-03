# Factlite

An embedded Rust library for a typed, SQLite-backed fact database with atomic
transactions and durable, revision-pinned reads. A thin CLI will exercise the
same library.

Factlite is being built incrementally. Repository setup is the current work;
database APIs are not implemented yet.

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

Remote hosting and licensing remain to be decided. Packages are not published.
