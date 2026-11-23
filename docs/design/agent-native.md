# Agent-native use: exploration

Status: exploration, written 2026-11-23 at the user's request. Nothing here is
decided or authorized to build. `AGENTS.md` still excludes a server and other
deferred scope. Each candidate below names the decision it needs.

The question: Factlite is a general-purpose embedded fact store. Could it also
be the store an agent uses for context, history, recall and durable work
queues? Datalevin is the reference point, because it ships an MCP server and
describes itself as AI-native.

## Findings in brief

- **History is what Factlite has and Datalevin does not.** Datalevin keeps
  current state only. Factlite's pinned snapshots, revision-ordered
  transactions, metadata and keyed retries cover agent history and
  "what changed since I last looked" with no new storage work.
- **Recall is what Datalevin has and Factlite does not.** Datalevin has
  full-text search, vector search and built-in embedding, all usable inside a
  Datalog query. Factlite matches by exact typed equality and ordered
  comparison only.
- **Datalevin's MCP server is a thin 1:1 adapter over its JSON API**, not a
  surface designed for a model. It is a useful list of what to keep and what
  to do differently.
- **Work queues are expressible today** with exact-prior-value expectations
  and keyed retries. The costs are permanent history under churn, no
  notification, and no store clock.
- **Permanence is the main hazard.** An agent that writes what it reads will
  eventually write a secret, and Factlite cannot remove it.
- **The first step is the one already designed:** the process interface in
  [`json-rpc.md`](json-rpc.md) with an MCP front end. Its two preconditions
  (roadmap items 3 and 4) were decided on 2026-11-06. It still needs the
  user's authorization.

## What Datalevin ships

Checked on 2026-11-23 against Datalevin 1.2.11 (released 2026-11-21): the
`master` branch source and docs, the changelog and datalevin.org. Source and
documentation were read; `dtlv` was not run. Links are under
[Sources](#sources).

### MCP server

- **Shipped** since 0.10.16 (2026-05-27) as a subcommand of the `dtlv` CLI:
  `dtlv mcp`. The only MCP flag is `--allow-writes`.
- **Transport:** stdio only, newline-delimited JSON-RPC, protocol version
  `2025-06-18`. It handles `initialize`, `ping`, `tools/list` and
  `tools/call`. Batches are rejected.
- **Capabilities:** tools only. No resources, no prompts, no server
  `instructions`. The MCP doc says it "is not designed around any specific
  branded host or coding-agent workflow".
- **Targets:** the server starts with nothing open. The model opens a local
  path or a `dtlv://user:pass@host/db` URI through a tool and carries the
  returned handle (`database-<uuid>`) on later calls. Any path the process can
  reach can be opened.
- **Tools:** 32, in six groups:
  - lifecycle: `open_database`, `close_database`, `open_kv`, `close_kv`,
    `api_info`;
  - Datalog reads: `query`, `datoms`, `search_datoms`, `count_datoms`, `pull`,
    `pull_many`, `entity`, `fulltext_datoms`;
  - Datalog write: `transact`;
  - key-value: `kv_get`, `kv_range`, `kv_transact`;
  - search index: open, close, add, remove, clear, indexed, count, search;
  - vector index: open, close, info, add, remove, indexed, search.
- **Read-only by default.** `--allow-writes` is one process-wide switch. Seven
  tools check it. The open tools do not: `open_database` passes `create` and a
  `schema` argument through in default mode. Read from source, not confirmed
  by running it.
- **Result limits:** 200 top-level items and 512 KiB per reply. Over the item
  cap the result is cut and `structuredContent.meta` reports
  `truncated: true` with the original count. Over the byte cap the result
  becomes a preview, then is omitted. The limits are not configurable from
  the CLI. There is no cursor or continuation token; `limit` and `offset`
  exist on three paged tools, and Datalog queries carry their own `:limit`.
- **Encoding:** JSON in `structuredContent`, duplicated as text. Keywords are
  strings with a leading colon, and any input string starting with `:` becomes
  a keyword. Large integers, dates, UUIDs and bytes use `~`-tagged objects.
  The `query` argument is EDN text inside a JSON string.
- **Discovery:** no schema tool, no list of databases or attributes, no
  explain. The underlying JSON API has `schema` and `explain` operations that
  MCP does not expose.
- **Timeouts:** no per-call deadline.

### Model, where it differs from Factlite

- **Current state only.** Retraction is deletion. There is no as-of read,
  history API or durable snapshot. The transaction log added in 0.10.7 is for
  replay and replication, and is garbage-collected (8 GiB or 7 days by
  default). Provenance annotations are on the roadmap for 2.2.0.
- **Schema is optional**, attributes can be added and migrated, and unique
  identity attributes upsert.
- **Transaction functions** (`:db/cas`, stored functions, `:db/ensure`
  post-conditions) and an explicit multi-statement transaction.
- **More than Datalog:** a key-value API and indexed documents (`idoc`) in
  the same store.
- **Server mode** with roles, read replicas and Raft failover.
- **Runtime:** a JVM library with native dependencies; `dtlv` is a GraalVM
  native image.
- **One writing process.** A WAL-mode directory cannot be shared between
  processes since 1.2.0, so a sidecar `dtlv mcp` beside an application that
  has the database open looks unsupported. That is an inference from the WAL
  doc, not a documented statement.

### Retrieval

- **Full-text:** its own engine, declared with `:db/fulltext true`, queried
  with the `fulltext` function. Boolean, phrase, fuzzy and prefix search.
- **Vector:** HNSW (usearch), declared with `:db.type/vec`, queried with
  `vec-neighbors`. The caller supplies vectors.
- **Built-in embedding** (0.10.16): `:db/embedding true` on a string attribute
  embeds at transaction time with a bundled llama.cpp and a model downloaded
  on first use, or an OpenAI-compatible endpoint. `embedding-neighbors` takes
  query text.
- **Composition:** all of these are query functions returning `[e a v]`
  tuples, so they join with ordinary clauses in one query, and the planner
  treats them as top-k access paths.

### Agent positioning

- The README and site say "AI-native". The author's post of 2026-01-19 argues
  that an agent needs "a context graph: entities, facts, relations,
  constraints, and memories that evolve over time".
- The book's agent-memory chapters are print only and were not read. The
  online afterword says confidence, source and supersession should be modelled
  as explicit facts, which follows from having no history.
- **No queue feature** is offered. Change notification (`listen!`,
  `listen-db!`) is in-process or ephemeral and is not exposed through MCP.

## What the MCP server shows

| Datalevin does | For Factlite |
| --- | --- |
| stdio, tools only, read-only unless a flag is passed | Keep. A read-only Factlite open is a read-only connection that cannot create or upgrade a file, so the default would hold at the storage layer. |
| Model opens paths and URIs and carries handles | Do differently. Bind named stores when the server starts. No path arguments, no `open`, `close` or `backup` tools. This removes filesystem reach and three tools of state the model must track. |
| 32 tools mirroring an internal API | Do differently. Tool definitions are sent to the model on every turn. About nine cover Factlite: `head`, `schema`, `query`, `pull`, `lookup`, `facts`, `history`, `transactions`, `transact`. |
| No schema discovery | Do differently. Factlite's schema is required and attribute definitions never change, so `schema` is a reliable description. Expose it as a tool and as an MCP resource. |
| No server instructions or prompts | Do differently. Ship `instructions` covering snapshots, expectations, idempotency keys and permanence. |
| Silent cut at 200 items, count reported, no continuation | Do differently. Every Factlite read already has a basis that survives later commits, so a cut result can name the basis and a continuation that returns the rest of the same answer. Datalevin cannot, because its state moves. |
| Query as EDN text in a JSON string | Open question. Factlite's query is a JSON object that an input schema can describe and the strict decoder validates. It is verbose and unfamiliar to a model. See candidate 8. |
| One global write switch | Keep the switch; add per-store read-only binding. |
| Credentials in tool arguments | Not applicable: no remote targets. |
| No per-call deadline | Factlite has a busy timeout for commits and none for queries. See candidate 9. |

## The four uses

### Context

What an agent loads at the start of a turn or task: a bounded set of related
facts, read consistently.

**Available now:**

- `head` returns a snapshot in about 0.02 ms
  ([measurements](../measurements.md)).
- Reads at that snapshot agree with each other and stay stable while other
  agents commit ([pinned views](../../crates/factlite/tests/pinned_views.rs)).
- `query` has ordering and a limit, so "the 20 most recent" is one read. A
  selective joined query took about 0.13 ms at 120,001 facts.
- `transactions` and `history` take an `after` revision. An agent that
  records the revision it last saw can ask for exactly what changed since.
  Datalevin has no equivalent.

**Gaps:**

- Pull is flat. Loading an entity with what it refers to takes one read per
  level. Nested pull is decided as a separate read API (decision brief 05) and
  not built.
- No disjunction. "Facts about X or Y" is two queries at one basis.
- Results are complete. There is no cap or continuation, so a model can
  receive more than it can use.
- The wire encoding tags every value (`{"string": "doing"}`). That is exact
  and costs tokens. A front end could return the strict form as structured
  content and a compact rendering as text. Not designed.

### History

What happened, in what order, and who did it.

**Available now**, and already used this way by the clajent kit through the
pod (plan step 10):

- Every commit is a revision with a report. Nothing is overwritten.
- As-of reads at any retained snapshot, including after reopen and from a
  backup ([revision reads](../../crates/factlite/tests/revision_reads.rs)).
- Per-entity and per-attribute history from the datom index: 0.12 ms for a
  three-event filter on the clajent store copy.
- Transaction metadata: a map of typed values on each commit, returned by
  `transactions`.
- Exact keyed retries: a repeated request returns the original report instead
  of committing twice
  ([exact retries](../../crates/factlite/tests/exact_retries.rs)). Models and
  agent harnesses retry often.

**Gaps:**

- **No clock.** Factlite records commit order, not time. "What happened
  yesterday" needs an `inst` the writer supplies, as a fact or as metadata.
- **Metadata is not queryable.** `query` matches facts only. Finding commits
  by agent or session means scanning `transactions`, or writing an event
  entity in each commit by convention.
- **Provenance is by convention.** Nothing records which agent wrote a commit
  unless the writer says so. A server front end could add it to metadata for
  every commit it accepts. That is adapter policy, not a storage change.

### Recall

Finding something without knowing its identity.

**Available now:** lookup by a unique value, exact typed equality joins, and
ordered comparisons. A string prefix is a range (`>= "foo"` and `< "fop"`).

**Gaps:** no full-text search, no substring match, no vector search. This is
the largest distance from Datalevin, and an agent memory without text search
is of limited use.

**Options:**

- **Full-text through SQLite FTS5.** The bundled SQLite build already
  compiles FTS5 in (`libsqlite3-sys` 0.40 sets `SQLITE_ENABLE_FTS5`), so this
  needs no new dependency. The index would be derived from the log like the
  datom index, and rebuildable. Joining matches to datom intervals would make
  search correct as of any revision, which Datalevin cannot offer. Open design
  questions:
  - how an index is declared, given that attribute definitions never change.
    It would have to be a derived-index setting outside the schema and outside
    history;
  - whether a full open validates it against the log, as it does the datom
    index, or treats it as unverified;
  - ranking. An index holding retracted values has different term statistics
    from one holding current values only;
  - whether search is a new clause in the query, or a separate read returning
    entities and a basis that the caller joins. The separate read is smaller.
- **Vector search.** Three sizes:
  - none: store embeddings as `bytes` and rank in the caller. Works today;
  - a brute-force nearest read in the library. Adequate for the tens of
    thousands of items an agent memory holds; not measured;
  - an index through the sqlite-vec extension, a new C dependency.

  In every case the caller supplies embeddings. Bundling a model runtime, as
  Datalevin does, would add a large dependency and a network download.
- **Outside the store.** Keep a search index elsewhere, keyed by entity and
  revision. Works today and leaves Factlite unchanged.

### Durable work queues

**Expressible now.** A job is an entity. Suggested attributes: `job/id`
(unique string), `job/state` (keyword), `job/owner` (string),
`job/lease-until` (inst), `job/run-after` (inst), `job/attempt` (i64) and a
payload.

- **Enqueue:** one commit. A second enqueue of the same `job/id` fails with
  `UniqueViolation`, because unique values do not upsert. An idempotency key
  makes a retried enqueue return the original report.
- **Claim:** query pending jobs with an order and a limit, pick one, and
  commit `state = claimed`, `owner` and `lease-until` with an expectation that
  `job/state` is exactly `{pending}`. Of two workers, one commits and the
  other gets `PreconditionFailed` and picks again.
- **Finish or fail:** a commit expecting `owner` to be this worker.
- **Lease expiry:** another worker queries claimed jobs whose `lease-until`
  is before its own clock, and reclaims with an expectation on the exact prior
  `lease-until`, so it loses to a concurrent heartbeat.
- **Delay and priority:** a `run-after` comparison and the query order.
- **Several worker processes on one file** are supported: commits serialize
  on SQLite's lock
  ([store concurrency](../../crates/factlite/tests/store_concurrency.rs), and
  the pod's contention test). Datalevin needs its server for this.
- **Audit comes free:** each job's full state history is retained.

Through the pod, a keyed 13-fact create took 0.52 ms and a conditional finish
0.23 ms at p50 on the clajent store copy.

**Costs and gaps:**

- **History is permanent.** Each job costs at least three revisions (enqueue,
  claim, finish), more with heartbeats, and none can be pruned. A writable
  open validates the whole log, measured at roughly 15 microseconds per
  revision on one workload. By extrapolation, not measurement, 100,000 jobs
  would put a writable open in the range of seconds. A long-lived server
  process pays that once. The file never shrinks.
- **No notification.** A worker polls `head` and reads `transactions` after
  the revision it last saw. Polling is cheap but adds latency equal to the
  interval.
- **No store clock.** Leases use the workers' clocks. On one host this is
  sound; the store cannot enforce it.
- **Contention at the head of the queue.** Every worker sees the same first
  pending job. Workers should pick at random among the first few. There is no
  equivalent of `SKIP LOCKED`.
- **No claim-next primitive.** Query then commit is two calls. Factlite has
  no transaction functions, by design: a request is data and validation is on
  the final state.
- **Not measured:** claim throughput with several processes contending, and
  behaviour at 100,000 or more revisions.

This fits agent-scale work: tasks, runs and handoffs in the thousands, where
the audit trail matters. It does not fit a high-throughput broker.

## Constraints that cross all four

- **Permanence.** Every value, metadata entry, temporary label and
  idempotency key is kept for the life of the lineage
  ([permanence](../permanence.md)). An agent that stores tool output or
  conversation text will store credentials and personal data sooner or later.
  Three mitigations exist and none is automatic:
  - keep bulk content outside the store and write a reference;
  - encrypt values and destroy the key, with the limits that document lists;
  - scope a store file to a session or project, and delete the file. A file
    is the only unit that can be forgotten. References do not cross files.
- **One host.** No server mode and no replication. Agents on several machines
  cannot share a store.
- **Authorized scope.** A server, a generic backend and new index types are
  all outside what `AGENTS.md` currently permits.

## Effect on the JSON-RPC and MCP design

[`json-rpc.md`](json-rpc.md) treats MCP as a second front end on the RPC
dispatch, decided separately. This exploration suggests the MCP front end
should differ from the RPC surface in these ways:

- **Preconditions are met.** Roadmap items 3 and 4 were decided on
  2026-11-06. The remaining gate is authorization.
- **No lifecycle tools.** Stores are bound by name at start-up. `open`,
  `close`, `backup` and `shutdown` stay RPC-only.
- **Result caps with a continuation.** The RPC design returns complete
  results and defers cursors. For a model, a cap is necessary. The design
  already states what a cursor must bind: snapshot, query, parameters and
  ordering.
- **Idempotency keys required on `transact`.** The RPC design lists this as
  an option worth adding. For a model it should be the default.
- **Provenance metadata** added by the server from its start-up
  configuration.
- **`schema` as a resource** and server `instructions`.
- **One server process per agent, one file.** Each agent launches its own
  stdio server. Cross-process commits already serialize correctly, so no
  shared daemon is needed.

## Candidates and the decisions they need

None is authorized. Sizes are rough.

| # | Candidate | Serves | Needs | Size |
| --- | --- | --- | --- | --- |
| 1 | Build `factlite-rpc` with an MCP front end as described above | all four | Authorization; `AGENTS.md` excludes a server. Also the two prerequisites `json-rpc.md` lists | large |
| 2 | Result caps and snapshot-bound continuation for reads | context | A cursor design; part of 1 or after it | medium |
| 3 | Nested pull as a read API | context | Already decided in brief 05; needs scheduling | medium |
| 4 | Full-text search through FTS5 | recall | Decisions on declaration, validation, ranking and query integration; a storage format change | large |
| 5 | Vector search | recall | A decision among the three sizes; a dependency decision for sqlite-vec | small to large |
| 6 | A queue guide with a tested schema and claim pattern, including a multi-process no-double-claim test | queues | Go-ahead only; no library change | small |
| 7 | A way to wait for a new revision | queues, context | A design: in-process only, or cross-process through SQLite's data version | medium |
| 8 | A Datalog text front end for queries | all four | A decision: a second query syntax to specify and keep in parity with the JSON form | medium |
| 9 | A query deadline | all four | A decision on the error and its outcome semantics | small |
| 10 | A convention or support for commit time and queryable provenance | history | A decision: convention only, adapter-stamped metadata, or metadata visible to `query` | small to medium |
| 11 | An agent guide: store-per-scope, the reference pattern, what never to write | all four | Go-ahead only | small |

## Suggested order

This is a recommendation, not a decision.

1. **6 and 11 first.** Both are documentation and tests on the existing
   library and pod. They show whether the queue pattern holds under
   contention before anything is built on it, and they put the permanence
   rule in front of agent authors.
2. **1, with 2 and 10's adapter-stamped metadata inside it.** This makes
   Factlite reachable by any MCP client and is where the design work above
   applies.
3. **4.** Text search is the gap that most limits recall, and FTS5 needs no
   new dependency.
4. **3, 7, 8, 9 and 5** when a consumer asks.

## Limits of this document

- Datalevin was read, not run. The read-only gating gap and the sidecar
  restriction are readings of source and docs.
- The Datalevin book's agent-memory chapters were not available.
- No Factlite measurement was taken for this document. Figures are from
  [measurements](../measurements.md). The queue figures at scale are
  extrapolated and say so.
- Datalevin's claims were gathered by a research subagent and spot-checked
  against downloaded source for the tool count, limits, write gating,
  capabilities and CLI flags. Other claims were not independently checked.

## Sources

Datalevin, `master` on 2026-11-23:

- [MCP doc](https://github.com/datalevin/datalevin/blob/master/doc/mcp.md),
  [MCP source](https://github.com/datalevin/datalevin/blob/master/src/datalevin/mcp.clj),
  [JSON API](https://github.com/datalevin/datalevin/blob/master/src/datalevin/json_api/shared.clj),
  [JSON conversion](https://github.com/datalevin/datalevin/blob/master/src/datalevin/json_convert.clj),
  [CLI source](https://github.com/datalevin/datalevin/blob/master/src/datalevin/main.clj).
- [README](https://github.com/datalevin/datalevin/blob/master/README.md),
  [changelog](https://github.com/datalevin/datalevin/blob/master/CHANGELOG.md),
  [transactions](https://github.com/datalevin/datalevin/blob/master/doc/transact.md),
  [WAL](https://github.com/datalevin/datalevin/blob/master/doc/wal.md),
  [server](https://github.com/datalevin/datalevin/blob/master/doc/server.md),
  [limits](https://github.com/datalevin/datalevin/blob/master/doc/limits.md).
- [Vector and embedding](https://github.com/datalevin/datalevin/blob/master/doc/vector.md),
  [full-text search](https://github.com/datalevin/datalevin/blob/master/doc/search.md),
  [documents](https://github.com/datalevin/datalevin/blob/master/doc/idoc.md),
  [query](https://github.com/datalevin/datalevin/blob/master/doc/query.md).
- [Online guide](https://datalevin.org/docs) (preface, mental model,
  afterword), and the author's post
  [Triple Store, Triple Progress](https://yyhh.org/blog/2026/01/triple-store-triple-progress-datalevin-posited-for-the-future/).

Factlite: [comparison](../comparison.md), [measurements](../measurements.md),
[permanence](../permanence.md), [JSON-RPC design](json-rpc.md),
[plan](../plan.md), [roadmap](../roadmap.md).
