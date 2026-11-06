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
