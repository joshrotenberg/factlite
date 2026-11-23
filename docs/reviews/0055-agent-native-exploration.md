# Review 0055: agent-native exploration

Branch `docs/agent-native-exploration`, PR #20. Documentation only. No human
reviewer and no remote CI ran.

## Method

- A Claude research subagent gathered Datalevin's MCP interface, model and
  retrieval features from primary sources (the `master` branch source and
  docs, the changelog, datalevin.org and the author's blog), returning each
  claim with its source.
- Self-review: the tool count, result limits, write-gated tools, capabilities
  and CLI flags were rechecked against downloaded source before the draft was
  written. Factlite figures were taken from `measurements.md`.
- An independent Claude subagent fact-checked the draft against the
  repository and the downloaded Datalevin files, without the research notes.
- `dtlv` 1.2.11, installed locally, was run over stdio in default mode against
  a scratch directory to test one claim (see finding 10).

## Independent fact-check findings

All thirteen were checked against the cited source and adopted.

1. **The described lease takeover could claim a finished job.** The draft had
   a takeover expect only the prior `lease-until` and a finish expect only
   `owner`. An expectation covers one attribute, so a late finish leaves the
   takeover's expectation true. Corrected: a takeover also expects
   `state = {claimed}` and the attempt; a finish expects state, owner and
   attempt.
2. **"One writing process" overstated Datalevin's limit.** The rule applies to
   WAL mode, which is off by default for embedded stores. Scoped to WAL mode;
   the non-WAL multi-process case is marked not checked; "Datalevin needs its
   server for this" removed.
3. **`store_concurrency.rs` is threads, not processes.** The multi-process
   claim now cites the CLI and pod process tests. Review 0054 made the same
   correction to the comparison.
4. **"Preconditions are met" overstated readiness of the JSON-RPC design.**
   The two decision preconditions are met. Two listed prerequisites are not
   done, one needs a decision, and the design predates items 3, 4 and 5.
   Corrected in the document, roadmap and status.
5. **The summary called the RPC interface "the first step"** while the
   suggested order put the guides first. Reworded to "the largest step".
6. **Nested pull was described as decided and awaiting scheduling.** Brief 05
   only placed it as a separate read API, on demand. Corrected.
7. **Wrong crate version:** FTS5 is enabled by `libsqlite3-sys` 0.38.2
   (through `rusqlite` 0.40), not 0.40. The substance held. Corrected.
8. **Result limits were incomplete:** the JSON API refuses results above
   10,000 items with an error, beneath the MCP truncation. Added. "Silent
   cut" removed, since truncation is reported.
9. **"1:1 adapter" was imprecise:** MCP exposes 32 of more than 130 JSON API
   operations, one tool each. Reworded. The judgment that it is "not designed
   for a model" was removed.
10. **The read-only gap was understated.** Opening without `create` also
    creates. Confirmed by running `dtlv mcp` in default mode:
    `datalevin_open_database` created a database directory at a nonexistent
    path both with `create: true` and without it, and `datalevin_transact`
    was refused. The document now says this was run.
11. **"Datalevin has no equivalent"** to reading changes since a revision was
    too absolute: in WAL mode its transaction log can be read from a
    position, bounded by retention. Corrected, with "no queryable snapshot".
12. **The open-time estimate for 100,000 jobs omitted live-state loading** and
    sat against the one-process-per-agent proposal. Now 5 to 15 seconds,
    still an extrapolation, with the per-launch cost stated.
13. **Smaller points, all corrected:** the release date (changelog 2026-11-20,
    published 2026-11-21); the book chapters are in print and ebook; as-of
    reads from a backup are tested in `backups.rs`; tool names carry a
    `datalevin_` prefix; the busy timeout applies to every connection;
    "most recent" needs a writer-supplied instant; `AGENTS.md` does not name
    index types; three opinions stated as fact were removed or qualified.

The fact-check found the remaining Datalevin and Factlite claims it checked
correct, including every cited measurement.

## Validation

`REQUIRE_BB=1 ./scripts/check.sh` passes locally, including the repository
link test. External links are not checked by any test.

## Limitations

- The Datalevin book's agent-memory chapters were not available.
- Whether several processes can share a non-WAL Datalevin directory was not
  checked.
- Whether `master` on 2026-11-23 equals the 1.2.11 release was not checked;
  the binary that was run reports 1.2.11.
- Queue costs at scale are extrapolated from measurements of other workloads.
- The candidate designs (FTS5 index, vector search, waiting for a revision)
  are proposals and were not prototyped.
- A separate local session exercised `dtlv` 1.2.11 over MCP and its notes
  agree on the tool count and protocol version. Those notes are outside this
  repository and were not reviewed.
