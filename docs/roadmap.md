# Roadmap

Candidate work that has not been authorized. Each entry says why it matters
and what decision it needs. When an entry is approved, it moves into
[`plan.md`](plan.md) as an item with acceptance criteria, and its outcome is
recorded in [`status.md`](status.md) and [`decisions.md`](decisions.md).
Nothing here is a commitment or a promise of order.

State as of 2026-11-05: `main` at `250bc6b` on the private GitHub remote,
storage format 4, plan items 0 through 11.5 complete. 181 tests pass locally
and in CI.

## 1. Publish (plan item 8.2): done

Published to the private repository `github.com/joshrotenberg/factlite` on
2026-11-05. CI passed there. A license is still unselected; it matters before
any public release, including item 2.

## 2. Pod release packaging

- **Why:** The clajent kit runs the pod from a local release build. Only macOS
  arm64 has been built and tested.
- **Decision needed:** Whether to publish to the babashka pod registry, and
  which platforms to build: macOS arm64 and x86_64, Linux x86_64 and aarch64,
  Windows x86_64.
- **Work once decided:** Per-platform builds with the babashka test run on
  each, versioned release tags, and a registry manifest. The `babashka-pod`
  repository already has release tooling for its own pods that could be
  reused.
- **Depends on:** a license, and a public repository or release host.

## 3. Bounded open time

- **Why:** Open replays and validates the whole retained log, so its cost
  grows with history and live state: about 0.9 s at 120,000 facts. Reads and
  commits no longer have this cost (items 9.2, 9.4, 9.5).
- **Decision needed:** How integrity is established when open does not replay
  everything. The options are:
  - **Verified checkpoints:** persist the validated state at a revision, verify
    the checkpoint itself, and replay only later revisions.
  - **An explicit `verify` operation:** open would do cheap structural checks
    only. Full validation becomes a separate CLI command and API, and
    corruption goes undetected until it runs.
  - **No change:** keep one open per process.
- **Note:** Checkpoints add derived persistent state that has to be checked
  against the log, as the datom index is.

## 4. Lookup refs in transactions

- **Why:** The last open request from the clajent kit. Writers could address an
  entity by a unique key (`[:run/key "A/run-3"]`) without first reading its ID.
- **Decision needed:** Lookup terms change the submitted request. Either they
  enter the canonical encoding as new term tags, so retries compare the lookup
  rather than the resolved ID; or they resolve before canonicalization, so
  retry identity depends on state. The first fits the existing contract better.
- **Also decide:** Whether a lookup that finds nothing fails `UnknownEntity`
  or may create the entity. Creating it would be an identity upsert, which the
  design excludes.

## 5. Query language

- **Why:** The query AST is conjunctive with aggregates (item 11.4).
- **Candidates:**
  - Negation (`not`) and disjunction (`or`).
  - Rules and recursion (SQLite recursive CTEs make this feasible).
  - Predicates on aggregate results (HAVING).
  - `avg`.
  - Nested pull patterns.
- **Decision needed:** Which ones, and their exact semantics. For example:
  what negation means for an attribute that is absent at the view's revision;
  `avg` rounding; and how recursion interacts with historical schema. The
  design deferred all of these because each changes correctness commitments.
- **Evidence standard:** Each addition needs reference-evaluator parity, as for
  existing queries and aggregates.

## 6. History retention and excision

- **Why:** All history is kept forever. There is no way to remove data that
  must not be retained, such as secrets written by mistake or personal data.
- **Decision needed:** Whether excision is ever allowed, and what it means.
  - For integrity: replay validation, retained reports and canonical retry
    evidence all assume complete history.
  - For snapshot tokens: views before an excision would change.
  - For backups.
- **Note:** This is a large semantic change. The design excluded pruning that
  discards semantic history.

## 7. Elixir NIF

- **Why:** The design names an Elixir server through a NIF as a possible
  consumer.
- **Decision needed:** Whether it is still wanted, and whether it should be a
  NIF (in-process, with crash isolation concerns) or a port or sidecar process
  like the babashka pod.
- **Note:** The pod showed that a thin adapter over the strict wire contract
  works. The same approach would apply.

## 8. Small operational gaps

None of these needs a design decision; each could be done directly.

- The CLI has no busy-timeout flag; the library and pod do (item 11.1).
- CI (`.github/workflows`) does not install babashka, so the pod's babashka
  tests skip there. Installing it and setting `REQUIRE_BB` would make them run.
- No query-planner statistics (`ANALYZE`) are kept. Plans rely on SQLite
  defaults, which were adequate for every measured query.
- CLI read commands open the file per process and so replay the whole log
  every time. They would benefit from 3.

## 9. Documentation

- **Why:** `status.md` is a growing per-item log (about 40 KB). The README's
  summary paragraph drifts from it, and `handoff.md` describes the state at
  item 8.1.
- **Decision needed:** Whether to keep `status.md` as the full log and add a
  short current-state page, or trim it.
