# pod-factlite

A [babashka](https://babashka.org) pod for Factlite: typed facts, atomic
declarative transactions and revision-pinned history over one SQLite file.

The pod is a thin adapter. Requests are translated from Clojure data into
Factlite's version-1 JSON wire requests and decoded by the library's strict
wire decoders, so the pod, the CLI and the Rust API share one set of rules.
It is built on the [`babashka-pod`](https://crates.io/crates/babashka-pod)
crate and speaks transit+json.

## Build and load

```sh
cargo build --release -p factlite-pod
```

```clojure
(require '[babashka.pods :as pods])
(pods/load-pod ["target/release/pod-factlite"])
(require '[pod.joshrotenberg.factlite :as f])
```

The pod has not been published to the pod registry.

## Values

Each Factlite type has exactly one Clojure representation, in both directions.

| Factlite | Clojure |
| --- | --- |
| `bool` | boolean |
| `i64` | long |
| `f64` | double (finite; `-0.0` becomes `0.0`) |
| `string` | string |
| `bytes` | byte array |
| `ref` | `{:id "e1..."}`, or `{:temp label}` inside transaction changes |

Anything else in a value position is rejected with `InvalidRequest`, never
coerced: nil, keywords, symbols, BigInts, BigDecimals, ratios, characters,
collections, insts and uuids. A long never equals a double: `1` and `1.0` are
different values and a filter of the wrong type fails `TypeMismatch`.

Attribute names, temporary labels, metadata keys and query input names may be
keywords or strings; the pod returns them as keywords. Maps that the pod
defines (transactions, snapshots, filters, options) take keyword keys only and
reject unknown keys.

## API

```clojure
(def conn (f/open "agents.sqlite"))   ; creates or opens; {:handle "factlite.store" :id 1}

(def report
  (f/transact conn
    {:idempotency-key "run-42"
     :schema [{:name :run/agent :type :string :cardinality :one :unique :none}
              {:name :run/status :type :string :cardinality :one :unique :none}
              {:name :run/tags :type :string :cardinality :many :unique :none}]
     :new-entities [:run]
     :changes [[:assert {:temp :run} :run/agent "codex"]
               [:assert {:temp :run} :run/status "started"]
               [:assert {:temp :run} :run/tags "nightly"]]
     :metadata {:session "abc"}}))

(def run (get-in report [:tempids :run]))  ; {:id "e1..."}

;; Conditional update: fails PreconditionFailed if the status changed.
(f/transact conn {:expect [[run :run/status ["started"]]]
                  :changes [[:assert run :run/status "done"]]})

(f/pull conn run [:run/status :run/tags])
;; => {:id "e1...", :run/status "done", :run/tags ["nightly"]}

(f/pull conn run [:run/status] {:at (:after report)})  ; or {:at 1}
;; => {:id "e1...", :run/status "started"}

(f/q conn '{:inputs {:status :string}
            :match [[?run :run/status $status] [?run :run/agent ?agent]]
            :where [(not= ?agent "")]
            :select [?run ?agent]
            :order [[?agent :asc] [?run :asc]]
            :limit 20}
     {:status "done"})
;; => {:columns [:run :agent], :rows [[{:id "e1..."} "codex"]]}
```

| Var | Arguments | Returns |
| --- | --- | --- |
| `open` | `[path]` `[path opts]` | a connection |
| `close` | `[conn]` | nil |
| `head` | `[conn]` | the latest snapshot |
| `transact` | `[conn tx]` | the transaction report |
| `schema` | `[conn]` `[conn opts]` | attribute definitions |
| `pull` | `[conn entity attrs]` `[conn entity attrs opts]` | `{:id ...}` plus present attributes, or nil |
| `facts` | `[conn]` `[conn filter]` `[conn filter opts]` | `[entity attr value]` vectors |
| `history` | `[conn]` `[conn filter]` `[conn filter opts]` | effective fact events |
| `transactions` | `[conn]` `[conn opts]` | committed reports, including no-change commits |
| `q` | `[conn query]` `[conn query params]` `[conn query params opts]` | `{:columns [...] :rows [...]}` |

### Connections

`open` creates the file if needed and returns a writable connection. Options:

- `:read-only true` opens an existing file through a read-only SQLite
  connection. Reads follow the latest head on each call; `transact` fails
  `InvalidRequest`. Read-only opens of files in an older format fail
  `UnsupportedFormat` until a writable open has upgraded them.
- `:busy-timeout-ms n` sets how long operations wait for another process's
  lock before failing `Busy` (default 2000; 0 fails at once).

### Transactions

Keys, all optional: `:schema`, `:new-entities`, `:changes`, `:expect`,
`:metadata`, `:expected-head`, `:idempotency-key`, and `:version` (default 1).

- Schema entries are `{:name :type :cardinality :unique}`. `:type` is one of
  `:bool :i64 :f64 :string :bytes :ref`, `:cardinality` is `:one` or `:many`,
  and `:unique` is `:none` or `:unique-one`.
- Changes are `[op entity attr value]` with op `:assert` or `:retract`. The
  entity is `{:id "e1..."}` or `{:temp label}` for a label in `:new-entities`.
- Expectations are `[entity attr values]`: exact typed set equality at the
  pre-transaction head, with `[]` meaning absent.
- `:expected-head` is a snapshot map; any intervening commit fails `HeadMismatch`.
- A retry with the same `:idempotency-key` and the same request returns the
  original report without a new revision; a changed request fails
  `IdempotencyConflict`. A pod request and the equivalent CLI JSON request
  have identical canonical bytes, so they share retry identity.

Reports have `:before`, `:after`, `:tempids` (label keyword to `{:id ...}`),
`:created-entities`, `:schema-added`, `:effective-delta` (`[op entity attr
value]`, retractions first) and `:metadata`.

### Reads and history

Every read takes an optional final opts map. `:at` is a snapshot map, such as
a report's `:after` or the result of `head`, or a revision number. A revision
number is resolved against this connection's store and lineage. A future
revision or a snapshot from another store fails `InvalidSnapshot`. Without
`:at`, the read captures head once.

`history` and `transactions` also take `:after` (default 0) and return events
or reports in `(after, revision]`. History events are maps with `:revision`,
`:ordinal`, `:op`, `:entity`, `:attr` and `:value`.

Filters for `facts` and `history` are maps with optional `:entity`, `:attr`
and `:value`.

Snapshots are `{:version 1 :store "s1..." :lineage "l1..." :revision n}`.

### Queries

A query map has `:match` and `:select` (required) and optional `:inputs`,
`:where`, `:order` and `:limit`. Quote it, as with other Datalog dialects.

- `?name` is a variable and `$name` an input declared in `:inputs` with a type
  keyword, such as `{:status :string}`. Other values are typed constants.
- Patterns are `[entity attr value]`; variables shared between patterns join.
- Predicates are lists `(op left right)` with op `=`, `not=`, `<`, `<=`, `>`
  or `>=`. Ranges compare values of one type only.
- `:select` and `:order` name variables; order entries are `[?var :asc]` or
  `[?var :desc]`. Include a total tie-breaker for stable limits.

Params are a map from input name to value: `{:status "done"}`.

## Errors

Every failure is an `ex-info` whose message is the library's message. Library
errors and pod argument errors have the same `ex-data` shape:

```clojure
{:code "PreconditionFailed"
 :message "..."
 :context {:entity {:id "e1..."} :attr :run/status
           :expected ["started"] :actual ["done"]}
 :commit-outcome "RolledBack"}
```

`:code` is one of Factlite's stable codes: `InvalidRequest`,
`UnknownAttribute`, `TypeMismatch`, `UnknownEntity`, `ConflictingBatch`,
`UniqueViolation`, `PreconditionFailed`, `HeadMismatch`, `IdempotencyConflict`,
`InvalidSnapshot`, `Busy`, `Storage`, `UnsupportedFormat`. `:commit-outcome`
is present for commit failures: `RolledBack`, or `Unknown` when COMMIT's
outcome could not be confirmed (retry with the same idempotency key).
Errors from the pod runtime itself, such as a closed connection, carry a
message only.

## Several processes

Any number of pod processes may open one file. Writers serialize on SQLite's
write lock. A request without `:expect` or `:expected-head` applies to the
current head, and a one-valued assertion replaces the current value. Use
expectations to reject stale writes, and reread before retrying
`PreconditionFailed`. `Busy` (the two-second lock wait expired) rolls back
and is safe to retry. Entities are never merged: every temporary label
allocates a new entity.

Opening a file validates its complete retained history, so open once per
process and keep the connection.

## Not exposed

Backups and the CLI's raw wire JSON are not exposed through the pod.
