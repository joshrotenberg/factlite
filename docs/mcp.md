# MCP server

`factlite-mcp` gives a model access to Factlite stores through the Model
Context Protocol, over stdio. Plan item 14.3; the design and its reasons are
in [`design/mcp.md`](design/mcp.md).

It runs on the same dispatch as the [JSON-RPC server](rpc.md), so reads,
writes, values and errors are the library's. What it adds is for a model:
stores named in advance, pages, a required retry key, and instructions.

Build with `cargo build --locked -p factlite-rpc`; the binary is
`target/debug/factlite-mcp`.

## Configure a host

Name each store and say whether the model may write it:

```text
factlite-mcp --writable-store notes=/path/to/notes.sqlite \
             --store archive=/path/to/archive.sqlite \
             --metadata agent=assistant
```

A host that reads MCP server definitions as JSON takes the same arguments:

```json
{
  "mcpServers": {
    "factlite": {
      "command": "/path/to/factlite-mcp",
      "args": ["--writable-store", "notes=/path/to/notes.sqlite", "--metadata", "agent=assistant"]
    }
  }
}
```

The model sees only the stores named here and cannot open any other file.

## Options

| Option | Meaning |
| --- | --- |
| `--store NAME=PATH` | Bind a store read-only. Repeatable. |
| `--writable-store NAME=PATH` | Bind a store that `transact` may write. Repeatable. |
| `--create` | Create a writable store whose file is missing. |
| `--validation full\|structural` | How read-only stores are opened. Structural skips replaying the log. Default `full`. |
| `--busy-timeout-ms MS` | How long to wait for another process's lock. Default 2000. |
| `--metadata KEY=VALUE` | Add this string to every transaction's metadata. Repeatable. |
| `--page-items N` | The most items in one page. Default 200. |
| `--page-bytes N` | The most bytes of items in one page; a page still holds one item when that item alone is larger. Default 262144. |
| `--max-line-bytes N` | The longest accepted message. Default 16 MiB. |

`NAME` is letters, digits, `_` or `-`. Every store is opened before the first
message is read. A store that cannot be opened stops the server with exit
code 2 and a message on stderr. A missing file is an error unless `--create`
is given and the store is writable; a read-only store is never created.

## Tools

| Tool | Arguments | Returns |
| --- | --- | --- |
| `head` | `store?` | `{snapshot, now}` |
| `schema` | `store?`, `at?`, `offset?`, `limit?` | `{basis, page, value}` |
| `facts` | `store?`, `filter?`, `at?`, `offset?`, `limit?` | `{basis, page, value}` |
| `pull` | `store?`, `entity`, `attrs`, `at?` | `{basis, value}` |
| `lookup` | `store?`, `attr`, `value`, `at?` | `{basis, value}` |
| `history` | `store?`, `filter?`, `after?`, `at?`, `offset?`, `limit?` | `{basis, page, value}` |
| `transactions` | `store?`, `after?`, `at?`, `offset?`, `limit?` | `{basis, page, value}` |
| `query` | `store?`, `query`, `at?`, `offset?`, `limit?` | `{basis, page, value}` |
| `transact` | `store?`, `tx` | report |

- `transact` is listed only when a store is bound writable, and accepts only
  writable stores.
- `store` may be omitted when one store is bound.
- The other arguments have the forms the [JSON-RPC guide](rpc.md#params)
  lists: tagged values, `{"id":"e1.…"}` entities, decimal-string revisions,
  and the CLI's request and query JSON.
- `now` is the server's clock as an `inst` value, for timestamps the model
  writes.
- A result is one text block of JSON. An error is a result with `isError`
  true and text `{"error":{code, message, context, commit_outcome?}}`, with
  the library's codes. Wrong arguments are reported the same way, with
  `InvalidRequest`.

## Pages

`schema`, `facts`, `history`, `transactions` and `query` return at most one
page: `--page-items` items and `--page-bytes` bytes, whichever comes first.
A page holds at least one item whenever one remains. `limit` makes a page
smaller. An offset past the end gives an empty page whose `offset` is the
total. `offset` and `limit` are written without a fraction or exponent.

`page` is `{offset, count, total}`, with `next` when items remain. To read the
next page, repeat the call with the same arguments, `at` set to the `basis`
of the first page, and `offset` set to `next`. An `offset` above zero without
`at` is refused, so every page comes from the same snapshot, whatever is
committed in between.

Each page computes the whole read and returns a slice of it. Query rows tile
across pages only when the query's `order` is total.

## Writes

- **Every transaction needs a non-empty `idempotency_key`.** The same request sent again
  returns the first report and commits nothing. The same key with a
  different request is refused with `IdempotencyConflict`.
- **`--metadata` is added to every transaction.** A request that sets one of
  those keys is refused. Because metadata is part of a request's identity, a
  retry must go to a server started with the same `--metadata`.
- **Facts are permanent.** The server's instructions tell the model not to
  write secrets, raw output, conversation text or personal data. See the
  [agent guide](agents.md).

## Resources

Each store's schema is a resource, `factlite://stores/NAME/schema`, read at the
latest revision.

## Protocol

- **Revisions:** `2024-11-05`, `2025-03-26`, `2025-06-18` and `2025-11-25`
  through `initialize`, and `2026-07-28`, whose requests carry their version
  in `_meta`, through `server/discover` or any request. Each request is
  answered in the form of the revision it names.
- **Capabilities:** tools and resources. No prompts, sampling, roots,
  logging or list-change notifications. A `subscriptions/listen` request in
  the `2026-07-28` form is acknowledged with nothing honored and stays open
  and silent until the client cancels it.
- **Batches** are answered entry by entry, as `2025-03-26` allows.
- **Not supported:** `null` or fractional ids, list cursors, HTTP.

## Tested

In `crates/factlite-rpc/tests/mcp_stdio.rs`, against the real binary: the
handshake of each revision and the per-request form of `2026-07-28`; protocol
errors, batches and notifications, including a write sent as one; each
tool's result against the library; pages of five reads, each read while
another connection commits, joined and compared with the whole result at the
first basis; pages filled up to the byte bound; read-only bindings, shown to
use a read-only connection by serving a damaged store with structural
validation; required non-empty keys, operator metadata and replays;
resources, including names that differ only in case; start-up arguments; and
the published input schemas against arguments the tools accept and refuse.
A unit test checks store names in the library type. The `2026-07-28` form is tested only against this
repository's own scripted client.
