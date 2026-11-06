# Comparison with related databases

Factlite compared with eight related systems, checked on 2026-11-06 against
each project's documentation, repository and release history. Projects change;
the versions checked are listed below, and each claim about another system
links to the page that supports it.

- **Datomic** (Pro 1.0.7745, Local 1.0.301): the model Factlite's design
  follows.
- **XTDB** (2.2.0): an immutable, bitemporal SQL database.
- **Datahike** (0.8.1913): a Datomic-style store with pluggable storage.
- **Datalevin** (1.2.10): a durable Datalog store on LMDB.
- **DataScript** (1.8.1): an in-memory Datalog database.
- **Mozilla Mentat** (0.11.1, archived in 2018): Datalog in Rust over SQLite,
  the closest relative in design.
- **CozoDB** (0.7.6): an embedded Datalog engine over relations.
- **SQLite** (3.54.1), used directly with history tables the application
  writes itself.

## Reading the tables

- **yes** and **no** are stated or shown by the project.
- **partial** means a limited or conditional form; the notes say how.
- **n.d.** means not documented: no statement either way was found. It is not
  a "no".
- **app** means the application can build it on what the system provides.
- A Factlite cell describes the current `main` branch. Its evidence is listed
  under [Factlite evidence](#factlite-evidence).

## Deployment

| | Factlite | Datomic | XTDB | Datahike | Datalevin | DataScript | Mentat | Cozo | SQLite |
|---|---|---|---|---|---|---|---|---|---|
| Embedded library | yes | partial [1] | partial [2] | yes | yes | yes | yes | yes | yes |
| Server mode | no | yes | yes | yes | yes | no | no | yes | no |
| Implementation | Rust | JVM | JVM | Clojure(Script) | JVM | Clojure(Script) | Rust | Rust | C |
| Storage | SQLite (WAL) | SQL, DynamoDB, Cassandra, local | disk or object store, plus a log | pluggable: memory, file, LMDB, RocksDB, S3, JDBC and others | LMDB fork | memory; optional pluggable store | SQLite | memory, SQLite, RocksDB, Sled, TiKV | own file |
| Concurrent writers | processes on one host, serialized by SQLite's lock | one transactor | one leader per database (Kafka when multi-node) | several processes per branch, fenced commits [4] | one process (threads batched), or through the server or HA leader | one process | SQLite's lock | depends on engine [5] | one at a time, one host |
| License | MIT or Apache-2.0 | Apache-2.0 binaries, closed source | MPL-2.0 | EPL-1.0 | EPL-2.0 | EPL-1.0 | Apache-2.0 | MPL-2.0 | public domain |
| Latest release | unpublished (0.1.0) | 2026-10 | 2026-10 | 2026-10 | 2026-10 | 2026-08 | 2018-08, archived | 2023-12 [3] | 2026-10 |

1. Datomic Local is an in-process library for small single-process
   applications; Pro peers run in the application but write through a
   transactor.
2. XTDB documents an in-process node for tests and the REPL; production use is
   a server.
3. Cozo's last commit to `main` was on 2024-12-04. The repository is not
   archived.

4. Datahike's default `:writer-ownership :shared` lets several processes
    commit to one branch, with conflicting commits retried through
    conditional writes on backends that support them; a single writer
    endpoint is optional.
5. Cozo's SQLite and in-memory engines allow one writer; RocksDB allows
    concurrent MVCC transactions within one process; TiKV is distributed.

## Data model

| | Factlite | Datomic | XTDB | Datahike | Datalevin | DataScript | Mentat | Cozo | SQLite |
|---|---|---|---|---|---|---|---|---|---|
| Model | typed facts (EAV) | EAV | documents in SQL tables | EAV | EAV, plus key-value and documents | EAV | EAV | relations | SQL tables |
| Schema required | yes | yes | no, inferred | optional | optional | optional | yes | yes, for stored relations | yes |
| Attribute definitions change | never | partial [6] | types widen | partial [7] | yes, with migration | partial [8] | partial | rewrite the relation | partial, `ALTER TABLE` |
| `1` and `1.0` distinct | yes | n.d. | stored distinct; SQL `=` n.d. | n.d. | coerced on write to the declared type | n.d. [9] | n.d. | distinct as keys; `==` treats them as equal | no |
| Cardinality-many | yes | yes | arrays | yes | yes | yes | yes | modeled with keys or lists | child tables |
| Unique values | yes, no upsert | yes, upsert | `_id` only | yes, upsert | yes, upsert | yes, upsert | yes, upsert | key columns, `:put` upserts | yes, `ON CONFLICT` |
| References | yes | yes | store an `_id` | yes | yes | yes | yes | n.d.; joins | foreign keys, off by default |

6. Datomic allows changing cardinality, uniqueness, `isComponent` and
   `noHistory`, never the value type.
7. Datahike allows changing cardinality and uniqueness.
8. DataScript can replace the schema (`reset-schema!`) without migrating data.
9. DataScript does not enforce value types. Its JVM index comparison treats
   `1` and `1.0` as equal, while query joins use Clojure equality, where they
   differ. This was read from source, not documentation.

## Transactions

| | Factlite | Datomic | XTDB | Datahike | Datalevin | DataScript | Mentat | Cozo | SQLite |
|---|---|---|---|---|---|---|---|---|---|
| Atomic multi-change transactions | yes | yes | yes, non-interactive | yes | yes | yes | yes | yes | yes |
| Conditional writes | exact prior value sets | `:db/cas`, transaction functions | `ASSERT` | `:db/cas`, transaction functions | `:db/cas`, `with-transaction` | `:db/cas`, transaction functions | no | `:ensure`, `:ensure_not` | app |
| Reject if anything changed since a basis | yes | n.d. | n.d. | n.d. | n.d. | n.d. | n.d. | n.d. | app |
| Built-in idempotent retries | yes, keyed | no; guidance [10] | n.d. | n.d. | partial [11] | n.d. | n.d. | n.d. | app |
| Lookup refs in writes | yes | yes | n.d.; rows are keyed by `_id` | yes | yes | yes | yes, own syntax | n.d.; rows are keyed | app, subquery |

10. Datomic documents that a timed-out transaction may have committed and
   suggests tagging transactions with a unique ID to check.
11. Datalevin's remote and HA clients can recover a lost reply when the request
   carries operation metadata. A key the application chooses is not
   documented.

## Time

| | Factlite | Datomic | XTDB | Datahike | Datalevin | DataScript | Mentat | Cozo | SQLite |
|---|---|---|---|---|---|---|---|---|---|
| As-of reads | yes | yes | yes | yes [12] | no | no [13] | no [18] | per relation [14] | app |
| History API | yes | yes | yes | yes [12] | no | no | transaction log | rows of the relation [14] | app |
| Time axes | commit order | transaction time | valid and system time | transaction time; valid time experimental | none kept | none kept | transaction time | application timestamps [14] | none |
| Reusable snapshot handle | token with store, lineage and revision | basis `t` | snapshot token | commit IDs (beta) | no | in-memory value only | n.d. | a timestamp | transient [15] |
| Remove data from history | no, by design [16] | excision (Pro) | `ERASE` | purge | no history kept | no history kept | n.d. | delete rows | app |

12. Datahike keeps history by default; a database created with
    `:keep-history? false` has none.
13. A DataScript database value is immutable while held in memory, but history
    is not tracked.
14. Cozo time travel applies to relations whose last key column has type
    `Validity`. The application supplies the timestamp, or uses the current
    time through `ASSERT` and `RETRACT`; the documentation leaves its meaning
    to the application. The Sled and TiKV engines do not support it.
15. SQLite's snapshot API needs a build option and WAL mode, and a checkpoint
    invalidates the snapshot.
16. See [permanence and sensitive data](permanence.md).

Factlite records revisions in commit order and stores no commit timestamps. An
application that needs wall-clock or valid time stores it as attribute values.

## Query

| | Factlite | Datomic | XTDB | Datahike | Datalevin | DataScript | Mentat | Cozo | SQLite |
|---|---|---|---|---|---|---|---|---|---|
| Language | Datalog-shaped AST (Rust, JSON, pod EDN) | Datalog | SQL, XTQL | Datalog | Datalog | Datalog | Datalog | Datalog (CozoScript) | SQL |
| Negation | `not`, `not-join` | yes | SQL | yes | yes | yes | yes | yes | yes |
| Disjunction | no | yes | SQL | yes | yes | yes | yes | yes | yes |
| Rules | no | yes | n.d. | yes | yes | yes | no [18] | yes | no |
| Recursion | no | n.d. [17] | no | yes | yes | yes | no [18] | yes | `WITH RECURSIVE` |
| Aggregates | count, count-distinct, sum, min, max, avg | many, custom | many | yes | many | many, custom | count, sum, min, max, avg | many | yes |
| Filter on aggregates (HAVING) | yes | n.d. | yes | n.d. | yes | n.d. | n.d. | app, chain rules | yes |
| Pull | flat | nested | nested | nested | nested | nested | flat | n.d. | app, JSON functions |
| Parameters | yes | yes | yes | yes | yes | yes | yes | yes | yes |

17. Datomic's current query reference says rules may invoke other rules but
    does not state that a rule may call itself.

18. Mentat was checked in source at its final commit: the query grammar has
    no rules, pull accepts only plain attributes and `*`, and no as-of read
    exists.

## Operations and interfaces

| | Factlite | Datomic | XTDB | Datahike | Datalevin | DataScript | Mentat | Cozo | SQLite |
|---|---|---|---|---|---|---|---|---|---|
| Online backup | yes, validated copy | yes (Pro) | object-store backup | export (experimental) | `dtlv copy` | serialize | n.d. | yes | yes |
| Integrity check | full replay on open by default; `verify` | `verify-backup` | n.d. | dump verification | n.d. | n.d. | version check on open | n.d. | `PRAGMA integrity_check` |
| CLI | yes | admin tools | server tools; query with `psql` | yes (beta) | yes | n.d. | yes | yes | yes |
| Versioned JSON wire format | yes, version 1 | no | no; Postgres wire, Flight SQL | unversioned HTTP | custom TCP | n.d. | n.d. | unversioned HTTP JSON | no |
| Babashka pod | yes | n.d. | through a Postgres pod | yes (beta) | yes | n.d. | n.d. | n.d. | third-party SQLite pod |
| Other languages | none | JVM | many, through Postgres drivers | Java, JS, Python, C (beta) | Java, Python, Node.js | JavaScript | C, Swift, Android | Python, Node.js, WASM, Java, Swift, Go, C | many |

## When another system fits better

Factlite is a small embedded store with strict typed facts, revision-pinned
reads and exact retries. It does not fit when an application needs:

- **Disjunction in queries:** every other system here.
- **Rules and recursion:** Datahike, Datalevin, DataScript and Cozo; Datomic
  documents rules, and SQLite has `WITH RECURSIVE`.
- **Valid time or bitemporal queries:** XTDB; Datahike has experimental valid
  time.
- **Deleting data from history:** Datomic Pro excision, XTDB `ERASE`, Datahike
  purge. Factlite keeps every committed value; see
  [permanence](permanence.md).
- **A server, several machines, or object storage:** Datomic, XTDB, Datahike,
  Datalevin, Cozo.
- **Graph algorithms, vector or full-text search:** Cozo, Datalevin, and
  Datahike (experimental graph algorithms, full-text and vector indices);
  SQLite has full-text search.
- **Nested pull:** most of the Datalog systems here.
- **A published, versioned package:** Factlite's crates are unpublished.
- **Languages other than Rust, a CLI or babashka:** Factlite has no other
  bindings.

Factlite fits an application that runs on one host, wants every commit kept
with reads pinned to an exact revision, and needs retries that return the
original result instead of applying twice. Of the systems here, only Factlite
documents keyed idempotent retries and a whole-database head fence together
with as-of reads.

## Factlite evidence

- Embedded Rust library over SQLite in WAL mode, CLI and babashka pod:
  [README](../README.md), [Rust guide](rust.md), [CLI](cli.md),
  [pod](../crates/factlite-pod/README.md).
- Several processes on one file, writes serialized:
  [concurrent CLI processes](../crates/factlite-cli/tests/cli_scenarios.rs),
  [concurrent pod processes](../crates/factlite-pod/tests/bb.rs),
  [pod guide, several processes](../crates/factlite-pod/README.md#several-processes).
- Typed values without coercion and immutable schema:
  [decisions](decisions.md#core-identities-and-values),
  [typed contract](../crates/factlite/tests/typed_contract.rs),
  [schema contract](../crates/factlite/tests/schema_contract.rs).
- Uniqueness without upsert (every temporary label allocates an entity):
  [decisions](decisions.md),
  [pod guide](../crates/factlite-pod/README.md#several-processes).
- Expectations, head fences, keyed retries and lookup refs:
  [exact retries](../crates/factlite/tests/exact_retries.rs),
  [transaction lookups](../crates/factlite/tests/tx_lookups.rs),
  [Rust guide](rust.md#retry-and-recover).
- As-of reads, history and snapshot tokens:
  [revision reads](../crates/factlite/tests/revision_reads.rs),
  [pinned views](../crates/factlite/tests/pinned_views.rs),
  [backups and lineage](backup.md).
- Queries, negation, aggregates and HAVING:
  [query evaluation](../crates/factlite/tests/query_evaluation.rs),
  [negation](../crates/factlite/tests/query_negation.rs),
  [aggregates](../crates/factlite/tests/query_aggregates.rs).
- Validation on open (full by default, structural on request for reads) and
  `verify`:
  [structural open](../crates/factlite/tests/structural_open.rs),
  [CLI validation and verify](cli.md#validation-and-verify).
- Not supported: `or`, rules, recursion, nested pull, valid time, excision.
  See the [roadmap](roadmap.md).

## Sources

Each list gives the pages the cells above rely on.

- **Datomic:**
  [overview](https://docs.datomic.com/datomic-overview.html),
  [storage](https://docs.datomic.com/operation/storage.html),
  [Local](https://docs.datomic.com/datomic-local.html),
  [schema reference](https://docs.datomic.com/schema/schema-reference.html),
  [schema change](https://docs.datomic.com/schema/schema-change.html),
  [transaction data](https://docs.datomic.com/transactions/transaction-data-reference.html),
  [transaction functions](https://docs.datomic.com/transactions/transaction-functions.html),
  [deployment and retries](https://docs.datomic.com/operation/deployment.html),
  [filters (as-of, history)](https://docs.datomic.com/reference/filters.html),
  [query reference](https://docs.datomic.com/query/query-data-reference.html),
  [pull](https://docs.datomic.com/query/query-pull.html),
  [backup](https://docs.datomic.com/operation/backup.html),
  [excision](https://docs.datomic.com/operation/excision.html),
  [REST (legacy)](https://docs.datomic.com/reference/rest.html),
  [Pro releases](https://docs.datomic.com/changes/pro.html).
- **XTDB:**
  [repository and releases](https://github.com/xtdb/xtdb/releases),
  [key concepts](https://docs.xtdb.com/concepts/key-concepts.html),
  [types](https://docs.xtdb.com/about/types-in-xtdb.html),
  [schema](https://docs.xtdb.com/reference/main/sql/schema.html),
  [transactions](https://docs.xtdb.com/reference/main/sql/txs.html),
  [transaction model and snapshot tokens](https://docs.xtdb.com/about/txs-in-xtdb.html),
  [queries](https://docs.xtdb.com/reference/main/sql/queries.html),
  [aggregates](https://docs.xtdb.com/reference/main/stdlib/aggregates.html),
  [storage](https://docs.xtdb.com/ops/config/storage.html),
  [log](https://docs.xtdb.com/ops/config/log.html),
  [databases](https://docs.xtdb.com/about/dbs-in-xtdb.html),
  [backup and restore](https://docs.xtdb.com/ops/backup-and-restore/overview.html),
  [drivers](https://docs.xtdb.com/drivers.html).
- **Datahike:**
  [repository](https://github.com/replikativ/datahike),
  [storage backends](https://github.com/replikativ/datahike/blob/main/doc/storage-backends.md),
  [configuration](https://github.com/replikativ/datahike/blob/main/doc/config.md),
  [secondary indices](https://github.com/replikativ/datahike/blob/main/doc/secondary-indices.md),
  [distribution and writers](https://github.com/replikativ/datahike/blob/main/doc/distributed.md),
  [schema](https://github.com/replikativ/datahike/blob/main/doc/schema.md),
  [time variance and purge](https://github.com/replikativ/datahike/blob/main/doc/time_variance.md),
  [valid time](https://github.com/replikativ/datahike/blob/main/doc/valid_time.md),
  [versioning](https://github.com/replikativ/datahike/blob/main/doc/versioning.md),
  [query engine](https://github.com/replikativ/datahike/blob/main/doc/query-engine.md),
  [export and import](https://github.com/replikativ/datahike/blob/main/doc/migration.md),
  [CLI](https://github.com/replikativ/datahike/blob/main/doc/cli.md),
  [HTTP routes](https://github.com/replikativ/datahike/blob/main/doc/http-routes.md),
  [babashka pod](https://github.com/replikativ/datahike/blob/main/doc/bb-pod.md),
  [releases](https://github.com/replikativ/datahike/releases).
- **Datalevin:**
  [README](https://github.com/datalevin/datalevin/blob/master/README.md),
  [no history or database values](https://github.com/datalevin/datalevin/blob/master/doc/alter.md),
  [transactions](https://github.com/datalevin/datalevin/blob/master/doc/transact.md),
  [WAL and writers](https://github.com/datalevin/datalevin/blob/master/doc/wal.md),
  [server](https://github.com/datalevin/datalevin/blob/master/doc/server.md),
  [query](https://github.com/datalevin/datalevin/blob/master/doc/query.md),
  [rules](https://github.com/datalevin/datalevin/blob/master/doc/rules.md),
  [dtlv CLI](https://github.com/datalevin/datalevin/blob/master/doc/dtlv.md),
  [install and pod](https://github.com/datalevin/datalevin/blob/master/doc/install.md),
  [changelog](https://github.com/datalevin/datalevin/blob/master/CHANGELOG.md).
- **DataScript:**
  [README](https://github.com/tonsky/datascript/blob/master/README.md),
  [storage](https://github.com/tonsky/datascript/blob/master/docs/storage.md),
  [changelog](https://github.com/tonsky/datascript/blob/master/CHANGELOG.md),
  [built-in aggregates](https://github.com/tonsky/datascript/blob/master/src/datascript/built_ins.cljc),
  [index comparison](https://github.com/tonsky/datascript/blob/master/src/datascript/db.cljc).
- **Mentat:**
  [repository](https://github.com/mozilla/mentat),
  [changelog](https://github.com/mozilla/mentat/blob/master/CHANGELOG.md),
  [connection and transactions](https://github.com/mozilla/mentat/blob/master/src/conn.rs),
  [transaction functions](https://github.com/mozilla/mentat/blob/master/db/src/tx.rs),
  [query grammar](https://github.com/mozilla/mentat/blob/master/edn/src/edn.rustpeg),
  [aggregates](https://github.com/mozilla/mentat/blob/master/query-projector-traits/aggregates.rs),
  [differences from Datomic](https://github.com/mozilla/mentat/wiki/Differences-from-Datomic).
  Rules, pull, conditional writes and as-of support were checked in source at
  the final commit, where none of rules, nested pull, conditional writes or
  as-of reads exist.
- **Cozo:**
  [repository](https://github.com/cozodb/cozo),
  [commits](https://github.com/cozodb/cozo/commits/main),
  [crate](https://docs.rs/cozo/latest/cozo/),
  [stored relations and transactions](https://docs.cozodb.org/en/latest/stored.html),
  [data types](https://docs.cozodb.org/en/latest/datatypes.html),
  [time travel](https://docs.cozodb.org/en/latest/timetravel.html),
  [queries](https://docs.cozodb.org/en/latest/queries.html),
  [aggregations](https://docs.cozodb.org/en/latest/aggregations.html),
  [backup and import](https://docs.cozodb.org/en/latest/nonscript.html),
  [graph algorithms](https://docs.cozodb.org/en/latest/algorithms.html),
  [vector search](https://docs.cozodb.org/en/latest/vector.html).
- **SQLite:**
  [release history](https://www.sqlite.org/changes.html),
  [when to use](https://www.sqlite.org/whentouse.html),
  [data types](https://www.sqlite.org/datatype3.html),
  [upsert](https://www.sqlite.org/lang_upsert.html),
  [foreign keys](https://www.sqlite.org/foreignkeys.html),
  [isolation](https://www.sqlite.org/isolation.html),
  [snapshot API](https://www.sqlite.org/c3ref/snapshot_open.html),
  [recursive queries](https://www.sqlite.org/lang_with.html),
  [aggregates](https://www.sqlite.org/lang_aggfunc.html),
  [backup](https://www.sqlite.org/backup.html),
  [PRAGMA](https://www.sqlite.org/pragma.html),
  [babashka SQLite pod](https://github.com/babashka/pod-babashka-go-sqlite3).
