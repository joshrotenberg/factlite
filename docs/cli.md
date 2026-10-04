# CLI version 1

Build with `cargo build --locked -p factlite-cli`; the binary is
`target/debug/factlite`. `--help` lists commands. Query execution is pending step 5.

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
factlite --db demo.sqlite transactions --after 0
factlite --db demo.sqlite transact --file start.json
```

The pulls return todo/doing. Retrying start returns the complete original B report
and creates no revision. Retrying create returns A's IDs/report without allocating
again. Changed content under the same key conflicts; a fresh stale conditional
request fails PreconditionFailed. Key identity preserves submitted operation
order and temporary labels. Retry the same request/key after an ambiguous response.

`--file -` reads a request from stdin. Reads accept mutually exclusive `--at`
compact tokens or `--at-file PATH|-` portable JSON snapshots; omission captures
head once. `facts` and `history` accept exact `--entity`, `--attr`, and tagged JSON
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
