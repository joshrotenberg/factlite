# CLI version 1

Build with `cargo build --locked -p factlite-cli`; the binary is
`target/debug/factlite`. `--help` lists commands.

Creation is explicit: `factlite init PATH` initializes or opens a supported file
and emits its full head snapshot. Other commands require an existing database.
Paths preserve platform-native encoding; typed flags and JSON require UTF-8.
Duplicate, unknown, missing, and command-inapplicable flags fail.

```sh
factlite init demo.sqlite
factlite --db demo.sqlite transact --file examples/create.json > A.report.json
jq '.after' A.report.json > A.json
task_id=$(jq -r '.tempids.task' A.report.json)

jq -n --arg task "$task_id" '{
  version: 1, idempotency_key: "demo-start-v1",
  expect: [{entity: {id: $task}, attr: "task/status", values: [{string: "todo"}]}],
  changes: [{op: "assert", entity: {id: $task}, attr: "task/status", value: {string: "doing"}}],
  metadata: {source: {string: "design-example"}}
}' > start.json
factlite --db demo.sqlite transact --file start.json > B.report.json
jq '.after' B.report.json > B.json

factlite --db demo.sqlite pull --at-file A.json --entity "$task_id" --attrs task/status
factlite --db demo.sqlite pull --at-file B.json --entity "$task_id" --attrs task/status
factlite --db demo.sqlite history --at-file B.json --entity "$task_id" --attr task/status --after 0
factlite --db demo.sqlite query --file examples/query.json --at-file A.json
factlite --db demo.sqlite query --file examples/query.json --at-file B.json
factlite --db demo.sqlite transactions --after 0
factlite --db demo.sqlite transact --file start.json
```

The pulls return todo/doing. The query returns no rows at A and the typed
task/title/Ada tuple at B. Retrying start returns the complete original B report
and creates no revision. Retrying create returns A's IDs/report without allocating
again. Changed content under the same key conflicts; a fresh stale conditional
request fails PreconditionFailed. Key identity preserves submitted operation
order and temporary labels. Retry the same request/key after an ambiguous response.

In a change, an entity may be `{"lookup":{"attr":"run/key","value":{"string":"A"}}}`
and a reference value `{"ref":{"lookup":{...}}}`. A lookup names the entity
that owns that value of a `unique_one` attribute before the transaction, so it
never sees values the same request asserts or moves. Checks run in order: the
attribute exists before the transaction (`UnknownAttribute`), it is
`unique_one` (`InvalidRequest`), the value has its type (`TypeMismatch`), and
an owner exists (`UnknownEntity`, context `{"lookup":{"attr","value"}}`). No
entity is created. Retry identity holds the lookup, so an exact retry returns
the original report after the value moves. Expectations, metadata, query
params and lookup values themselves take no lookups.

`--file -` reads a request from stdin. Reads accept mutually exclusive `--at`
compact tokens or `--at-file PATH|-` portable JSON snapshots; omission captures
head once. Read commands use a read-only database connection. `facts` and `history` accept exact `--entity`, `--attr`, and tagged JSON
`--value` filters. `history`/`transactions` default `--after` to 0, excluding that
revision and including the pinned basis. Pull's comma-separated `--attrs` list
projects exactly those names; an empty string projects none.

Database commands emit one versioned JSON value plus newline on stdout. Pull
matches the design's flat entity/attributes envelope; an unknown entity emits
`{"version":1,"entity":null}`. Many fields are typed-value arrays; absent fields
are omitted. Schema/facts/history/transactions use version plus respectively
schema/facts/events/transactions arrays. Events retain decimal revision/ordinal
strings; transaction entries contain revision and the full original report.
Numeric values keep their tags and string representations; bytes emit padded
standard base64. Help/version are the two human-readable output commands.

Failures leave stdout empty before response writing and emit one JSON error on
stderr with code, message, typed context, and available commit_outcome evidence.
Do not infer rollback from a lost/failed stdout write after a commit; retry using
the original key/request. Source errors remain available through Rust APIs.

| Exit | Meaning |
| --- | --- |
| 0 | Success |
| 2 | Invalid request, value/type/entity/attribute, or snapshot input |
| 3 | Batch/uniqueness/precondition/head/idempotency conflict |
| 4 | SQLite busy timeout; retryable |
| 5 | Storage/I/O failure or unsupported stored format |

The `assert_cmd` fixture exercises these commands as separate processes. It
checks complete report/read parity with Rust, no-change/retry behavior, all value
types, structured failures, busy bounds, paths with spaces, and Unix non-UTF8 paths.

## Queries

`query --file PATH|- [VIEW]` accepts the envelope in `examples/query.json`.
`query.match` and `query.select` are required arrays. Inputs, where, order, and
outer params default empty; limit defaults absent. Explicit null is invalid.
Only one input may consume stdin: query and snapshot cannot both use `-`.

A match is `[entity-term, "namespace/attribute", value-term]`. Terms are
`{"var":"name"}`, `{"param":"name"}`, or a concrete tagged value; entity terms
must have Ref type. Temporary references are transaction input only. Declared
inputs use bool/i64/f64/string/bytes/ref/keyword/inst and all must be supplied
in params. Since format 4, `{"keyword":"status/done"}` is a keyword value and
`{"inst":"1730851200000"}` an instant in signed decimal milliseconds since the
Unix epoch; neither equals a string or i64 with the same text or number.

Predicates use `{"left":TERM,"op":"eq","right":TERM}` with eq/ne/lt/le/gt/ge.
Equality is exact typed equality; ranges require the same type. Variables in
predicates and select must be pattern-bound; selected names must be unique.
Order entries use `{"field":"selected-name","direction":"asc"}` or desc.
Limit is a nonnegative JSON integer through u64::MAX; zero still validates.

Results are `{"version":1,"columns":[...],"rows":[[TAGGED_VALUE,...],...]}`.
Entity columns use Ref. Projection removes duplicate tuples before ordering and
limit. Supply a total order, such as title plus task identity, for predictable
limited results. Missing facts fail a pattern; no null values are invented.

Aggregates are optional `query.aggregates` entries
`{"fn":"sum","var":"cost","name":"total"}`, with fn count, count_distinct,
sum, avg, min or max, plus optional `query.with` variable names. The query projects
the selected, `with` and aggregated variables, removes duplicate tuples, groups
by the selected variables, aggregates, then orders and limits. Aggregate names
follow the selected columns and may be ordered. Include the entity in `with`
when equal values from different matches must each count. Count results are
i64; sum keeps its i64 or f64 type, and overflow or a non-finite f64 total is
an error. avg takes i64 or f64 and returns f64: an i64 mean is the exact sum
divided by the count, rounded once to the nearest f64 (ties to even); an f64
mean is the ascending-order f64 sum divided by the count. No matching tuples
give no rows.

`query.having` is an optional array of predicates, in the `where` encoding,
applied to each aggregated row before ordering and the limit. A `var` in
HAVING names a result column (a selected variable or an aggregate name), not
a pattern variable; constants and params work as in `where`. Count columns are
i64, avg is f64, and sum, min and max keep the variable's type. HAVING needs at
least one aggregate. Every group is aggregated first, so an overflow in a
group HAVING would drop still fails the query; a limit of zero returns no rows
without aggregating.

Negation is optional `query.not`, an array of groups
`{"join":["task"],"match":[...],"where":[...]}` whose `match` and `where`
default empty. A row survives only if no group holds for it at the view's
revision. Without `join` the group is `not`: every variable in it must be
bound by the outer patterns. With `join` it is `not-join`: the listed
variables must be bound outside, and every other variable is local to the
group, must be bound by one of its patterns, must not reuse an outer
variable's name, and is invisible outside. `"join":[]` correlates on nothing.
Absence of any value is
`{"join":["task"],"match":[[{"var":"task"},"task/owner",{"var":"owner"}]]}`.
A group needs at least one pattern or predicate, groups do not nest, and an
attribute unknown at the view is `UnknownAttribute`, not trivially absent.

## Backup

`backup --to PATH` emits the full head token copied by SQLite and creates a
validated standalone file at a new destination. It copies all retained history,
including WAL commits, without replacing an existing path or sidecar. Read it
with the usual schema/facts/pull/history/transactions/query commands. See
[backup and lineage rules](backup.md) before resuming writes from a restored copy.

Committed values, metadata and retry records are permanent, and a backup copies
all of them through its captured revision. See [permanence and sensitive data](permanence.md) before storing
data that may have to be deleted. Error JSON on stderr can include values from
the request.

## Busy timeout

`init` and every database command accept `--busy-timeout-ms MS`: how long to
wait for another process's lock before failing with exit code 4 (`Busy`). The
default is 2000; 0 fails at once. The value must be unsigned decimal
milliseconds no greater than 2147483647. Read commands use read-only
connections; under WAL a reader does not wait for a writer's lock, but opening
can still wait briefly, for example while another process switches a new file
to WAL.

## Validation and verify

Read commands accept `--validation full|structural` (default `full`). A
structural open checks the format markers, the table and index sets, and that
the head matches the latest transaction, without replaying the log. That takes
a few milliseconds instead of time proportional to history. Corruption is then
found only by `verify`; until then reads can fail with `Storage` or return
answers from the stored index.

`factlite --db PATH verify` fully validates the latest committed state and
prints the verified snapshot. It exits 5 on corruption or a missing file.

