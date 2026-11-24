# MCP front end: design

Status: authorized by the user on 2026-11-23 with the JSON-RPC interface, built
as plan item 14.3. The user guide is [`../mcp.md`](../mcp.md).

`factlite-mcp` is a Model Context Protocol server over stdio. It is a second
front end on the dispatch in `crates/factlite-rpc`: each tool call becomes one
dispatch call, and the result is that call's result. It adds what a model
needs and a program does not: named stores instead of paths, a page size, a
required retry key, and instructions.

The reasons for each difference from Datalevin's MCP server are in
[`agent-native.md`](agent-native.md), "What the MCP server shows".

## Stores are bound when the server starts

```text
factlite-mcp --store NAME=PATH ... [--writable-store NAME=PATH ...] [options]
```

- **`--store NAME=PATH`** binds a store read-only: a read-only connection
  that cannot create the store, upgrade it or write to it. SQLite still
  creates its `-wal` and `-shm` files beside an existing store, so the
  directory must be writable.
- **`--writable-store NAME=PATH`** binds a store that `transact` may write.
- **`--create`** lets a writable store be created when its file is missing.
  Without it a missing file is an error at start-up.
- **No tool takes a path.** There is no `open`, `close` or `backup` tool. A
  model can reach only the files the person who configured the server named.
- **Every store is opened before the first message is read.** A store that
  fails to open stops the server with a message on stderr and a non-zero exit.
  A writable store pays a full validating open each start; a read-only store
  can be opened with `--validation structural`.
- **`store`** is an argument of every tool. It may be omitted when exactly one
  store is bound.

## Tools

Nine, the dispatch's methods minus the lifecycle ones:

| Tool | Arguments | Result |
| --- | --- | --- |
| `head` | `store?` | `{snapshot, now}` |
| `schema` | `store?`, `at?`, `offset?`, `limit?` | `{basis, value, page}` |
| `facts` | `store?`, `filter?`, `at?`, `offset?`, `limit?` | `{basis, value, page}` |
| `pull` | `store?`, `entity`, `attrs`, `at?` | `{basis, value}` |
| `lookup` | `store?`, `attr`, `value`, `at?` | `{basis, value}` |
| `history` | `store?`, `filter?`, `after?`, `at?`, `offset?`, `limit?` | `{basis, value, page}` |
| `transactions` | `store?`, `after?`, `at?`, `offset?`, `limit?` | `{basis, value, page}` |
| `query` | `store?`, `query`, `at?`, `offset?`, `limit?` | `{basis, value, page}` |
| `transact` | `store?`, `tx` | report |

- `transact` is listed only when a writable store is bound.
- Arguments other than `store`, `offset` and `limit` are the JSON-RPC params
  of the same name, decoded by the same wire decoders.
- Each tool's input schema lists the bound store names, so the model sees
  which stores exist without a discovery call.
- Every tool states all four behaviour hints. Reads are read-only. `transact`
  is not read-only, is destructive (a retraction or a replacement changes
  the current state, though every earlier state stays readable), and is
  idempotent (see below).
- **`head` also returns `now`,** the server's clock as an instant. A store has
  no clock and a model has none either, and the [agent guide](../agents.md)
  asks writers to record time. This is the one result that is not a library
  result.

## Results are text

A tool result is one text block holding compact JSON. There is no
`structuredContent` and no output schema. A result that carried both would be
sent twice, and hosts differ on which one they show the model.

An error is a result with `isError: true` whose text is
`{"error":{code, message, context, commit_outcome?}}`, the Factlite error
object. Wrong or missing arguments are reported the same way, with the code
`InvalidRequest`, so the model can read what to change. Only an unknown tool
name, or `arguments` that is not an object, is a protocol error (-32602).

## Pages

A model cannot take an unbounded result. `schema`, `facts`, `history`,
`transactions` and `query` return at most a page of items:

- **Page size:** `--page-items` (200) items and `--page-bytes` (262,144) bytes
  of items, whichever is reached first. A page holds at least one item
  whenever one remains, even an item larger than the byte bound. `limit`
  lowers the item count for one call. It cannot raise it. An offset past the
  end gives an empty page whose `offset` is the total.
- **`page`** is `{offset, count, total}`, plus `next` when items remain.
- **To continue,** repeat the call with the same arguments, `at` set to the
  result's `basis`, and `offset` set to `next`.
- **A continuation is bound to a snapshot.** `offset` above zero is refused
  without `at`. Every page is then a slice of one result at one basis, however
  many commits happen between calls. This is what Datalevin's server cannot
  offer: its state moves.
- **Each page computes the whole read and returns a slice.** The cost of a
  page is the cost of the read. The item count is known, so `total` is exact.
- **Order.** Facts, history, transactions and schema have a defined order.
  Query rows have one only when the query's `order` is total; pages of a
  query with a partial order are not guaranteed to tile.
- `pull`, `lookup` and `head` return one item and are not paged.

A snapshot-bound cursor that avoided recomputing would need a cursor format
that binds the query, its parameters and its order, as `json-rpc.md` says. The
offset form binds them by having the client send them again, and is enough
for a model reading a few pages.

## Writes

- **A retry key is required.** `transact` without `idempotency_key` is
  refused. A model that is unsure whether a call went through can send it
  again unchanged and get the first report.
- **Provenance is added by the server.** Each `--metadata KEY=VALUE` given at
  start-up is added to the metadata of every transaction as a string. A
  request that sets one of those keys itself is refused. The model cannot
  forge them or forget them.
- **The server adds no time.** Metadata is part of a request's identity, so a
  time stamped per attempt would turn every retry into a conflict. The model
  takes `now` from `head` once per step and writes it itself.
- **A retry must reach a server started with the same metadata,** for the
  same reason.

## The schema is also a resource

`factlite://stores/NAME/schema`, one per store. The name is in the path,
which is case-sensitive, not in the authority, which a client may lowercase, read at the head. Its text is the
`schema` tool's result without a page cut. Hosts that show resources to the
person or attach them to context can use it without a tool call.

## Instructions

The `initialize` and `server/discover` results carry `instructions`: which
stores are bound and which are writable, how tagged values look, that reads
name a `basis` and take `at`, how pages continue, that writes need a key and
should carry `expect`, and that nothing written can be deleted, so secrets,
raw tool output and personal data must not be written.

## Protocol

- **Transport:** stdio, one JSON-RPC message per line, the line reader and
  envelope parser of `factlite-rpc`. Stdout carries only protocol messages.
- **Two eras, chosen per request.** The specification's 2026-07-28 revision
  removed the `initialize` handshake: every request carries its protocol
  version and client capabilities in `params._meta`, and `server/discover`
  describes the server.
  - A request whose `_meta` names protocol version `2026-07-28` is answered
    in that revision's form: `resultType`, server information in `_meta`, and
    cache hints on list and read results.
  - Any other request is answered in the handshake form, for the revisions
    `2024-11-05`, `2025-03-26`, `2025-06-18` and `2025-11-25`. `initialize`
    answers with the requested revision when it is one of those, and with
    `2025-11-25` otherwise.
  - The server keeps no state between requests in either era.
- **Methods:** `initialize`, `server/discover`, `ping`, `tools/list`,
  `tools/call`, `resources/list`, `resources/read`,
  `resources/templates/list`.
- **Notifications** are accepted and ignored.
- **Batches** are answered entry by entry. `2025-03-26` allows them and
  later revisions removed them; a client of a later revision does not send
  them.
- **Ids** are strings or integers. A null or fractional `id` is refused with
  -32600, as the specification requires.
- **`initialize`** requires `capabilities` and `clientInfo` objects; the
  stateless form requires `clientCapabilities` to be an object.
- **No list is paged,** so any `cursor` is refused with -32602.
- **Unknown members of MCP params are ignored,** because the protocol adds
  them between revisions. Unknown members of a tool's `arguments` are refused.
- **Capabilities:** `tools` and `resources`, with no list-changed or
  subscribe support. No prompts, logging, sampling or roots.

## Not built

- A Datalog text form for queries (candidate 8 in the exploration).
- A wait for a new revision, nested pull, search (candidates 7, 3, 4, 5).
- A query deadline (candidate 9). A tool call runs until its read finishes.
- HTTP transport.

## Sources and limits

The protocol facts above were taken from the specification pages at
`modelcontextprotocol.io/specification` on 2026-11-23, read through a
subagent. The 2026-07-28 form was written from those pages and tested only
against this repository's own scripted clients, not against a client
implementation of that revision.
