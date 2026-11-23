# Roadmap

Candidate work that has not been authorized. Each entry says why it matters
and what decision it needs. When an entry is approved, it moves into
[`plan.md`](plan.md) as an item with acceptance criteria, and its outcome is
recorded in [`status.md`](status.md) and [`decisions.md`](decisions.md).
Nothing here is a commitment or a promise of order.

For the current state, see the summary at the top of [`status.md`](status.md).

## 1. Publish (plan item 8.2): done

Published to the private repository `github.com/joshrotenberg/factlite` on
2026-11-05. CI passed there. The user chose MIT OR Apache-2.0 on 2026-11-05;
the crates and README carry it.

## 2. Pod release packaging: decided, not now

The user decided on 2026-11-05 not to publish to the babashka pod registry.
The pod stays in this repository and is built from source
(`cargo build --release -p factlite-pod`), as `crates/factlite-pod/README.md`
describes. Revisit if another machine or user needs prebuilt binaries; only
macOS arm64 has been built and tested so far.

## 3. Bounded open time: decided

Decided 2026-11-06 ([brief](decision-briefs/03-bounded-open-time.md)): full
validation by default, an opt-in structural open for read-only connections,
and an explicit verify. Plan item 12.1.
## 4. Lookup refs in transactions: decided

Decided 2026-11-06 ([brief](decision-briefs/04-transaction-lookup-refs.md)):
option A1, lookups in changes and reference values, plan item 12.3 (done). Later:
expectation lookups with canonical request version 2.
## 5. Query language: decided in stages

Decided 2026-11-06 ([brief](decision-briefs/05-query-language.md)):

- `not` and `not-join` now (plan item 12.2, done);
- `avg` and HAVING (plan item 12.5, done);
- `or` and recursion only on demand;
- nested pull as a separate read API.
## 6. History retention and excision: decided

Decided 2026-11-06 ([brief](decision-briefs/06-history-excision.md)): history
stays immutable. Document the permanence rule and the application-level
encryption pattern (plan item 12.4, done in
[permanence](permanence.md)). Design a lineage rewrite only if a real
deletion requirement appears. In-place excision is ruled out.
## 7. Elixir NIF: deferred

The user deferred this on 2026-11-05. When it returns, the open questions are
whether it should be a NIF or a port or sidecar process like the babashka pod,
and which consumer needs it.

## 8. Small operational gaps

None of these needs a design decision; each could be done directly.

- Done 2026-11-06: the CLI accepts `--busy-timeout-ms`, and CI installs
  babashka 1.13.229 and runs the pod's babashka tests with `REQUIRE_BB`.
- No query-planner statistics (`ANALYZE`) are kept. Plans rely on SQLite
  defaults, which were adequate for every measured query.
- CLI read commands open the file per process and so replay the whole log
  every time. They would benefit from 3.
- Done 2026-11-06: `crates/factlite/tests/doc_links.rs` checks that every
  relative link in the repository's Markdown resolves, as part of
  `cargo test`.

## 9. Documentation: done

Done 2026-11-06. `status.md` keeps its full per-item log, which `AGENTS.md`
requires. It gains a short current-state section at the top that replaces the
drifting summary. The README overview and `handoff.md` are rewritten for the
current state.

## 10. JSON-RPC interface: designed, deferred

- **Why:** One process-boundary protocol for every language. A long-lived
  process opens a store once, and the same dispatch can back an MCP server.
  The library stays the core.
- **Design:** [`design/json-rpc.md`](design/json-rpc.md). It would live in this
  workspace as `crates/factlite-rpc`, a dispatch library and a stdio server
  binary over the strict wire contract. No such crate exists yet.
- **When to build:** only after items 3 (open-time integrity) and 4 (lookup
  refs in transactions) are decided. A ready consumer (Elixir, or agents
  through MCP) raises the priority but does not skip that condition. Building
  it needs the user's authorization, like the pod did.
- **Prerequisites that can land earlier:** public wire decoders for read
  requests (entity, filter, revision and lookup params, which the CLI parses
  itself today); and a defined CLI outcome when a commit succeeds but writing
  its report fails. The latter changes the CLI's exit-code contract, so it
  needs a decision.

## 11. Agent-native use: explored, first candidates chosen

- **Why:** the user asked on 2026-11-23 whether Factlite can also serve as an
  agent's store for context, history, recall and durable work queues, with
  Datalevin's MCP interface as the reference.
- **Exploration:** [`design/agent-native.md`](design/agent-native.md). History
  is covered by what exists. Recall lacks full-text and vector search. Work
  queues are expressible with expectations and keyed retries, at the cost of
  permanent history under churn. Permanence is the main hazard for agent
  writers.
- **Candidates:** eleven, listed there with the decision each needs. The
  largest are building item 10 with an MCP front end, and full-text search
  through SQLite FTS5. The smallest are a tested queue guide and an agent
  guide, which need no library change.
- **Decided 2026-11-23:** candidates 6 and 11, the queue guide with a tested
  claim pattern and the agent guide, move into the plan as step 13. Done:
  [queues](queues.md) and [agents](agents.md).
- **Decision needed:** which other candidates, if any, to move into the plan.
  Item 10's decision preconditions (items 3 and 4) are met; it still needs
  authorization, its two listed prerequisites and a design revision.

## 12. Head reads that join attributes of finished entities: found, undecided

- **Why:** measuring the queue pattern (plan 13.1) showed that the read
  behind every claim grows by about 0.8 to 1.2 ms per 1,000 finished jobs in
  the file, from 0.22 ms with none to 6.6 ms and 9.8 ms with 8,000. It
  follows the jobs ever processed, not the jobs pending. Any workload that
  queries for the few entities in one state, joined to attributes that the
  many entities which left that state still hold, will see the same. Figures
  and method: [measurements](measurements.md), "Work queue pattern".
- **What is known:**
  - Every read that joined two or more of the job attributes grew at about
    the same rate. A read of the single pattern `job/state = pending` grew
    about fourteen times more slowly: 0.46 ms at 8,000 finished jobs.
  - A reviewer's reproduction of the generated SQL on plain SQLite found the
    join driven from the due-time range, probing state per job. The plan used
    by the bundled SQLite was not captured.
  - In an experiment outside the repository, an index on
    `(attribute, value, retracted, entity, asserted)` cut one form of the
    query from 1.99 ms to 0.21 ms at 4,000 finished jobs and did not help the
    form that also matches an attribute every job holds. `ANALYZE` alone
    changed nothing. Pattern order in the query changed the time by a third.
- **Candidate remedies, none tried in the library:** make the generated SQL
  follow the query's pattern order, so the author can put the selective
  pattern first; keep planner statistics; add an index over open intervals by
  attribute and value. The first needs no storage format change.
- **Decision needed:** whether to investigate. The first step would be to
  capture the real query plan.
- **Until then:** the [queue guide](queues.md) says to start a new store file
  after a few thousand jobs.
