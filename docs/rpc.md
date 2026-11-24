# JSON-RPC server

`factlite-rpc` serves the library to another process over stdio, as JSON-RPC
2.0. It is for a client in any language that wants a store open for longer
than one command. Plan item 14.2; the design and its reasons are in
[`design/json-rpc.md`](design/json-rpc.md).

It is a thin adapter. Requests carry the same version-1 JSON as the
[CLI](cli.md), results come from the same library encoders the CLI prints
with, and every rule about facts, snapshots and retries is the library's.

Build with `cargo build --locked -p factlite-rpc`; the binary is
`target/debug/factlite-rpc`.

## A session

One request per line on stdin, one response per line on stdout. Diagnostics
go to stderr. The process ends at end of input or after `shutdown`.

```jsonl
{"jsonrpc":"2.0","id":1,"method":"open","params":{"path":"demo.sqlite","create":true}}
{"jsonrpc":"2.0","id":2,"method":"transact","params":{"handle":"h1","tx":{"version":1,"idempotency_key":"setup-v1","schema":[{"name":"task/id","type":"string","cardinality":"one","unique":"unique_one"},{"name":"task/status","type":"keyword","cardinality":"one","unique":"none"}],"new_entities":["t"],"changes":[{"op":"assert","entity":{"temp":"t"},"attr":"task/id","value":{"string":"T-1"}},{"op":"assert","entity":{"temp":"t"},"attr":"task/status","value":{"keyword":"status/todo"}}]}}}
{"jsonrpc":"2.0","id":3,"method":"lookup","params":{"handle":"h1","attr":"task/id","value":{"string":"T-1"}}}
{"jsonrpc":"2.0","id":4,"method":"transact","params":{"handle":"h1","tx":{"version":1,"idempotency_key":"start-T-1","changes":[{"op":"assert","entity":{"lookup":{"attr":"task/id","value":{"string":"T-1"}}},"attr":"task/status","value":{"keyword":"status/doing"}}]}}}
{"jsonrpc":"2.0","id":5,"method":"history","params":{"handle":"h1","filter":{"attr":"task/status"}}}
{"jsonrpc":"2.0","id":6,"method":"query","params":{"handle":"h1","query":{"version":1,"query":{"match":[[{"var":"t"},"task/status",{"keyword":"status/doing"}],[{"var":"t"},"task/id",{"var":"id"}]],"select":["id"]},"params":{}}}}
{"jsonrpc":"2.0","id":7,"method":"close","params":{"handle":"h1"}}
```

The first response names the handle and the head it opened at:

```json
{"jsonrpc":"2.0","id":1,"result":{"handle":"h1","head":{"lineage":"l1.…","revision":"0","store":"s1.…","version":1}}}
```

A read answers with the snapshot it used and the CLI's output for that read:

```json
{"jsonrpc":"2.0","id":3,"result":{"basis":{"lineage":"l1.…","revision":"1","store":"s1.…","version":1},"value":{"entity":{"id":"e1.….0000000000000001"},"version":1}}}
```

A test runs the requests above as written and checks that each one succeeds.

## Methods

Params are always an object. A member marked `?` is optional. Unknown members
are refused.

| Method | Params | Result |
| --- | --- | --- |
| `open` | `path`, `create?`, `read_only?`, `validation?`, `busy_timeout_ms?` | `{handle, head}` |
| `close` | `handle` | `{}` |
| `head` | `handle` | snapshot |
| `verify` | `handle` | the snapshot it validated |
| `transact` | `handle`, `tx` | report |
| `schema` | `handle`, `at?` | `{basis, value}` |
| `facts` | `handle`, `filter?`, `at?` | `{basis, value}` |
| `pull` | `handle`, `entity`, `attrs`, `at?` | `{basis, value}` |
| `lookup` | `handle`, `attr`, `value`, `at?` | `{basis, value}` |
| `history` | `handle`, `filter?`, `after?`, `at?` | `{basis, value}` |
| `transactions` | `handle`, `after?`, `at?` | `{basis, value}` |
| `query` | `handle`, `query`, `at?` | `{basis, value}` |
| `backup` | `handle`, `to` | snapshot of the copy |
| `shutdown` | none; `params` may be omitted | `{}` |

### Params

| Member | Form |
| --- | --- |
| `tx` | A transaction request, as `transact --file` takes it. See the [CLI guide](cli.md). |
| `query` | A query envelope, as `query --file` takes it. |
| `at` | A snapshot: the object a report or `head` gives, or the compact token string (`d1.…`) the CLI's `--at` takes. Absent: the head, captured once for this call. |
| `entity` | `{"id":"e1.…"}`. Temporary labels and lookup references belong to transactions. |
| `attr` | An attribute name, `"namespace/name"`. |
| `attrs` | An array of attribute names. `[]` projects no fields. Unlike the CLI's comma-separated `--attrs`, it can name an attribute whose name contains a comma. |
| `value` | A tagged value, such as `{"i64":"3"}`. |
| `filter` | An object with any of `entity`, `attr` and `value`. Absent, or `{}`, matches every fact. |
| `after` | A revision as a decimal string, such as `"12"`. Absent: `"0"`. Events and transactions after it, up to the basis, are returned. |
| `path`, `to` | A path on the server's filesystem. A relative path is taken from the server's working directory. |
| `create`, `read_only` | Booleans. Absent: false. |
| `validation` | `"full"` (the default) or `"structural"`. |
| `busy_timeout_ms` | An integer from 0 to 2147483647. Absent: 2000. A larger integer is refused by the library with `InvalidRequest`. |

### Results

- A **snapshot** is `{"version":1,"store":…,"lineage":…,"revision":"…"}`.
- A **report** is the transaction report the CLI prints.
- In `{basis, value}`, `basis` is the snapshot the read used and `value` is
  the CLI's output for that read, version member included.

## Handles

- `open` returns a handle, a short string naming one connection. Names are
  not reused within a process.
- **`open` creates no store unless `create` is true.** A path that does not
  exist fails with `Storage`: operation `open database` for a writable open,
  as in the CLI's `transact`, and SQLite's own error for a read-only one, as
  in the CLI's reads. With `create`, an existing store is opened as it is.
  Three cases fall outside the rule:
  - an existing empty file is initialized as a new store, with or without
    `create`, as the CLI's commands do;
  - opening an existing store, even read-only, can leave SQLite's `-wal` and
    `-shm` files beside it;
  - the check that the file exists runs just before the library opens it, so
    a file removed in between is created. The CLI has the same gap. Closing
    it needs an "existing store only" open in the library.
- **`read_only`** opens a connection that cannot create, upgrade or write.
  `transact` and `backup` on it fail with `InvalidRequest`. A read that names
  no snapshot, and `head`, use the latest head at the time of the call.
- **`validation: "structural"`** skips replaying the log at open and needs
  `read_only`. `verify` validates fully whenever asked.
- Opening one path twice gives two independent connections.
- `close` releases a handle. Any later request naming it fails with
  `InvalidRequest`.

## Rules

- **Every request needs an `id`.** A request without one is a notification,
  which JSON-RPC forbids answering. The server does not execute it and notes
  it on stderr. An `id` of `null` is an id.
- **The `id` comes back exactly as sent,** byte for byte. It is a string, a
  number or null.
- **The `id` is not a retry key.** After a timeout, a lost response or a
  `commit_outcome` of `Unknown`, send the same `tx` again with the same
  `idempotency_key`. If the first attempt committed, the original report comes
  back and nothing new is committed. A request with no key cannot be
  recovered this way: sending it again may commit twice. Start the server with
  `--require-idempotency-key` to refuse unkeyed transactions.
- **A batch is not a transaction.** An array of requests gets an array of
  responses, one per request that has an `id`, plus one -32600 response with
  a null `id` for each entry that is not a request object. `transact`,
  `open`, `close`, `backup` and `shutdown` are refused inside a batch, each
  with its own -32600 response; the other entries run, in order. An empty
  batch gets one -32600 response, not an array, and a batch of notifications
  gets no line at all. One `transact` with one request is the only atomic
  unit.
- **Reads report their basis.** Two reads that name no snapshot can see
  different revisions. To read one state, pass the first read's `basis` as
  `at` to the rest. A snapshot does not fence a write; use `expect` or
  `expected_head` in the transaction.
- **Results are complete.** There is no pagination. Bound a read with a
  filter, `after`, or a query `limit`.
- **One request at a time.** The server reads the next line after it has
  written the previous response. A commit waiting for another process's lock
  delays the requests behind it by up to the busy timeout.

## Errors

Clients branch on `error.code` for protocol failures and on `error.data.code`
for Factlite failures, never on a message.

| Code | Meaning |
| --- | --- |
| -32700 | The line is not JSON, not UTF-8, or longer than the limit. `id` is null. |
| -32600 | Not a request object: a wrong or missing `jsonrpc` or `method`, an `id` of another kind, an unknown or repeated member, an empty batch, or an order-dependent method inside a batch. `id` is null except for the last case. |
| -32601 | The method does not exist. |
| -32602 | `params` is missing or not an object; a member is missing, repeated or unknown; or `handle`, `path`, `to`, `create`, `read_only`, `validation` or `busy_timeout_ms` is not of its form: a string, a boolean, `"full"` or `"structural"`, a non-negative integer. |
| -32000 | A Factlite error. `error.data` is `{code, message, context, commit_outcome?}`, the same object the CLI prints under `error`. |

`error.data.code` is one of the library's codes: `InvalidRequest`,
`UnknownAttribute`, `TypeMismatch`, `UnknownEntity`, `ConflictingBatch`,
`UniqueViolation`, `PreconditionFailed`, `HeadMismatch`,
`IdempotencyConflict`, `InvalidSnapshot`, `Busy`, `Storage` and
`UnsupportedFormat`. `Busy` is retryable, as elsewhere.

Members that carry the wire contract (`tx`, `query`, `at`, `entity`, `attr`,
`attrs`, `value`, `filter`, `after`) are decoded by the library exactly as
the CLI decodes them, so a malformed one is a Factlite `InvalidRequest` or
`InvalidSnapshot` under the request's own `id`, not a protocol error.

## Options

```text
factlite-rpc [--max-line-bytes N] [--require-idempotency-key]
factlite-rpc --help | --version
```

- **`--max-line-bytes N`:** the longest accepted line, 16 MiB by default. A
  longer line is dropped as it is read and answered with -32700.
- **`--require-idempotency-key`:** refuse a `transact` whose request has no
  key.
- **Exit codes:** 0 at end of input or after `shutdown`, 2 for invalid
  arguments, 5 when reading stdin or writing stdout fails.

Blank lines within the limit are skipped; a longer line of spaces gets -32700
like any other. A last line needs no newline.

## What it is not

- **Not a network service.** There is no listener and no authentication. The
  client is the process that started the server, and `open` and `backup` act
  on any path that process's user can reach.
- **Not a shared daemon.** Each client starts its own server. Several server
  processes can write one file; SQLite serializes their commits, as it does
  for any other Factlite processes on one host.
- **No answer survives the process.** If the server dies, or its output pipe
  closes, after a commit and before the response is read, the client has no
  outcome. A keyed retry through a new server returns the report.
- **Backups keep the store's identity.** Open a copy read-only, or restore it
  as [backups](backup.md) describes. Writing to both the original and a copy
  makes two histories under one identity, and the server cannot detect that.

## Tested

In `crates/factlite-rpc/tests/`:

- **`dispatch_parity.rs`:** 1,024 random traces of writes and reads through
  the dispatcher, on a writable and a read-only handle, each result and each
  error compared with the library's on the same file.
- **`stdio_protocol.rs`:** the rules above against the real binary: ids,
  notifications, batches, malformed and oversized lines, params, wire members
  whose errors must equal the library decoders' own, end of input,
  `shutdown`, a closed output and arguments.
- **`stdio_stores.rs`:** a session compared read by read with the library;
  every refusal code; creation; read-only handles; closed handles; a busy
  store; a keyed retry after the server is killed; and four server processes
  committing to one file.

Not tested: a commit whose outcome is `Unknown`, which needs the library's
test-only fault hooks (its own suite covers the retry); and the CLI binary
itself. The tests compare with the library, whose encoders the CLI also
uses.
