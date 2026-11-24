# JSON-RPC interface: design

Status: designed 2026-11-05, authorized by the user and revised on 2026-11-23,
built as plan step 14. This document fixes the shape of Factlite's
process-boundary interface. What the 2026-11-23 revision changed is listed at
the end.

## Purpose and scope

The library stays the core. JSON-RPC 2.0 is the primary interface for other
processes: one protocol that any language can speak, a long-lived process that
opens a store once, and a base for an MCP server. It replaces nothing in the
library. The CLI and the babashka pod keep their own interfaces. Both may later
call the same dispatch code.

It lives in this workspace as `crates/factlite-rpc`:

- **Library:** a dispatch layer that takes a method name and raw JSON params and
  returns a JSON result or error. It holds no storage semantics.
- **Binary:** `factlite-rpc`, which serves dispatch over a transport.
- **Second binary:** `factlite-mcp`, the MCP front end (plan 14.3).

Like the CLI and pod, it is a thin adapter over the library and the strict wire
contract in `crates/factlite/src/wire/`.

## When to build

The condition was that roadmap items 3 (open-time integrity) and 4 (lookup refs
in transactions) be decided first, because both change what this interface
exposes. Both were decided on 2026-11-06 and built as plan 12.1 and 12.3.

Later items are not all additive. Item 5 (query language) changes the `query`
envelope's semantics. Item 6 (excision) would change what snapshot tokens and
retained retry reports guarantee, and so would weaken the coordinated-read and
retry rules below. Revisit this design if either is adopted.

## Transport and framing

- **Stdio first:** one JSON-RPC message per line on stdin and stdout. Stdout
  carries only protocol messages; diagnostics go to stderr.
- **Messages:** each line must be valid UTF-8 and no longer than a configured
  maximum (default 16 MiB, `--max-line-bytes`). An oversized or malformed line
  gets a -32700 response with a null `id`, and the server continues. An
  oversized line is discarded as it is read, not buffered.
- **Blank lines** (empty or only JSON whitespace) are skipped without a
  response.
- **Responses** are one line each, written and flushed before the next line
  is read.
- **Optional later:** a local Unix socket for several clients of one process.
  No TCP; the design excludes network promises.
- **Trust:** `open` and `backup` take paths on the server's filesystem and act
  with the server process's permissions. Clients are trusted local processes,
  such as the parent that launched the server. There is no authentication.

## Lifecycle and handles

- `open` returns a `handle`: an opaque string naming one open connection. The
  name differs from the `store` field in snapshot tokens.
- `open` never creates a file unless `create: true` is passed, so a mistyped
  path fails instead of making an empty store. This matches the CLI, which
  creates stores only through `init`. A missing path fails with Factlite
  `Storage` and the operation `open database`, as in the CLI. `create` with
  `read_only` is refused.
- A writable `open` upgrades older formats in place, as `Store::open` does. A
  `read_only: true` open fails `UnsupportedFormat` on older formats, as
  `Db::open_read_only` does.
- `validation` is `"full"` (the default) or `"structural"`. Structural skips
  replaying the log at open and is accepted only with `read_only`, as in the
  library. `verify` validates fully later.
- On a read-only handle, `transact` and `backup` fail with Factlite
  `InvalidRequest`, and `head` returns the latest head through `Db::at_head`.
- Opening the same path twice gives two handles with separate connections, the
  same as two processes.
- `close` waits for in-flight requests on that handle, then releases it. Later
  requests on it fail `InvalidRequest`.
- The process ends at end of input or on `shutdown`. SQLite makes every commit
  either complete or absent, but a commit interrupted by a crash or SIGKILL
  leaves the client without an outcome. It recovers with a keyed retry (see
  Rules).

## Methods

Params are always a JSON object. Unknown members are refused. Snapshots (`at`)
use the wire snapshot object, or the compact token string that `SnapshotId`
prints and the CLI's `--at` takes. Revisions (`after`) use decimal strings,
entities `{"id": "e1..."}`, attribute names plain strings, and values the wire's
tagged values. `tx` and `query` embed the existing wire request envelopes
unchanged, so lookup refs, negation, aggregates and `having` need nothing here.
`busy_timeout_ms` is a JSON integer.

| Method | Params | `result` of the response |
| --- | --- | --- |
| `open` | `path`, `create?`, `read_only?`, `validation?`, `busy_timeout_ms?` | `{handle, head}` |
| `close` | `handle` | `{}` |
| `head` | `handle` | snapshot |
| `verify` | `handle` | snapshot it validated |
| `transact` | `handle`, `tx` | wire report |
| `schema` | `handle`, `at?` | `{basis, value}` |
| `facts` | `handle`, `filter?`, `at?` | `{basis, value}` |
| `pull` | `handle`, `entity`, `attrs`, `at?` | `{basis, value}` |
| `lookup` | `handle`, `attr`, `value`, `at?` | `{basis, value}` |
| `history` | `handle`, `filter?`, `after?`, `at?` | `{basis, value}` |
| `transactions` | `handle`, `after?`, `at?` | `{basis, value}` |
| `query` | `handle`, `query`, `at?` | `{basis, value}` |
| `backup` | `handle`, `to` | snapshot of the copy |
| `shutdown` | none | `{}` |

`filter` is `{entity?, attr?, value?}`. A read's `value` is the existing wire
output for that read, unchanged. `basis` is the snapshot the read used. Adding
`basis` beside the wire output leaves the version-1 wire outputs, which the CLI
and pod consume, unchanged.

`backup` copies keep the store and lineage identities and retained retry
evidence, exactly as `Store::backup` does. Open a copy read-only, or use it only
for the faithful, retired-writer restore described in `docs/backup.md`. Opening
the source and a copy both writable creates unsupported divergent history. The
server cannot detect this across processes.

## Rules

**Batches.** A batch of requests gets one response per request, except
notifications, as the specification requires. Entries may execute in any order
and are never a transaction. Methods whose effect depends on order (`transact`,
`open`, `close`, `backup` and `shutdown`) are refused inside a batch. Each such
entry gets its own -32600 error response, and the other entries are processed
normally. Atomicity comes only from one `transact` call with one complete
request.

**Writes need an `id`.** The specification forbids replying to a notification,
even with an error. A `transact` notification (no `id` member) is therefore not
executed and gets no response. The server logs it to stderr. The same applies
to every other method: no notification is executed, because none has an effect
worth having without its reply. A request with `"id": null` is a request, not a
notification, and is answered.

**Ids are echoed exactly.** An `id` is a string, a number or null, and the
response repeats its text byte for byte, so a large or fractional number comes
back as sent. Any other kind fails as -32600 with a null `id`.

**Retries.** The `id` only correlates a response with its request; it is not an
idempotency key. After a timeout, a dropped connection or a `commit_outcome` of
`Unknown`, a client recovers by retrying the same request with the same
`idempotency_key`. That returns the original report if the first attempt
committed. Without a key the outcome cannot be recovered: a retry is a new
request and may create a second revision. Clients that may retry should always
send a key. With `--require-idempotency-key` the server refuses a `transact`
whose request has none, with Factlite `InvalidRequest`.

**Reads report their basis.** A read without `at` captures head once, for that
call only. Two such reads can see different revisions. Coordinated reads pass
the `basis` of the first as `at` to the rest. A snapshot is not a write fence;
writes still use `expect` or `expected_head`.

**Strict parsing.** Every message line is parsed with a duplicate-rejecting
parser, so a duplicate `id`, `method` or `params` member fails as -32600. A
duplicate or unknown member of `params` fails as -32602. Within params, wire
envelopes and values go through the wire decoders on their raw text, so
duplicate keys, unknown fields and malformed tagged values fail exactly as in
the CLI, as Factlite errors. Factlite's request `version` stays inside its
envelope; `"jsonrpc": "2.0"` does not version Factlite semantics.

The parser keeps each member's text and never rebuilds it, so nothing is
reinterpreted on the way to a wire decoder.

**Concurrency.** Requests arriving on one stdio stream are answered in order,
on one thread: the next line is read after the previous response is written.
Each handle has one SQLite connection behind a mutex, as `Store` and `Db` do
today. Requests therefore serialize, and a commit waiting on the busy timeout
delays everything behind it. Parallel reads would need extra
read connections per handle. That is a later option with its own validation,
not part of this design.

**Result size.** `facts`, `history`, `transactions` and `query` return complete
results; there is no pagination. Callers bound them with filters, revision
bounds and query `limit`. Cursor pagination is deferred, as in the original
design, and must bind snapshot, query, parameters and ordering.

## Errors

- **Protocol failures** use the standard codes.
  - -32700: a line that is not JSON, not UTF-8, or too long.
  - -32600: an invalid request object, a duplicate member, or an
    order-dependent method inside a batch.
  - -32601: an unknown method.
  - -32602: params that are missing or not an object; a missing, duplicate
    or unknown member; or a member this interface defines (`handle`, `path`,
    `to`, `create`, `read_only`, `validation`, `busy_timeout_ms`) of the wrong
    JSON kind. A member the wire contract defines is decoded by the wire and
    fails as a Factlite error.
- **Factlite errors**, including wire decoding failures inside params, use code
  -32000. `error.message` is the library message. `error.data` is the inner
  error object that `wire::encode_error` produces under its `"error"` member:
  `{code, message, context, commit_outcome?}`, without the outer version
  wrapper. Clients branch on `error.data.code`, never on the message or the
  numeric JSON-RPC code.
- **Busy** is a Factlite error (`error.data.code` `Busy`), retryable as
  elsewhere.

## Library and adapter work this design depends on

- **Public read-request decoding.** Done as plan 14.1: `wire::decode_entity_id`,
  `decode_attr`, `decode_attrs`, `decode_filter` and `decode_revision`, with
  encoders. The dispatch layer parses only the JSON-RPC envelope and the
  top level of `params` itself.
- **CLI commit outcome on output failure.** The CLI can commit and then fail
  while writing the report to stdout. It reports that as exit code 5, the same
  as a real storage failure. A distinct outcome changes the CLI's exit-code
  contract and needs a decision. It is still undecided and does not block this
  interface. The server has the same gap in another form: a commit can
  succeed and the response be lost with the pipe. A keyed retry recovers the
  report either way, but only if the request had a key.

## MCP

MCP is JSON-RPC 2.0. The MCP server exposes the same dispatch as MCP tools
(`transact`, `query`, `pull` and so on) on the same transport. It is a second
front end on `factlite-rpc`'s dispatch, not a separate implementation.
Authorized with this interface on 2026-11-23 and built as plan 14.3; its design
is `mcp.md` beside this file, written with that item.

## Testing when built

- **Parity:** every method gives results identical to the library and CLI for
  the same wire input, including all errors.
- **Specification:**
  - A notification `transact` gets no response and commits nothing.
  - A batch with `transact` gets an error for that entry and normal responses
    for the others.
  - A duplicate member fails as -32600.
  - Oversized and invalid lines fail as -32700, and the server keeps serving.
- **Recovery:** a keyed retry after a dropped connection returns the original
  report. An injected `Unknown` outcome needs the library's test-only fault
  hooks, which no other crate can reach; the library's own fault suite covers
  the retry after `Unknown`, and the server only passes the error through.
- **Handles:** `open` without `create` on a missing path fails; read-only
  handles refuse writes; requests after `close` fail.
- **Contention:** several server processes commit to one file, as in the pod's
  contention test.
- **Transport:** `assert_cmd` scenarios over stdio.

## Reviews

- **Codex second opinion on the idea (2026-11-05, through kit).** It
  recommended against JSON-RPC as the core interface, suggesting an optional
  adapter built only on demand. Its conditions are adopted: no batch atomicity,
  writes not sent as notifications, idempotency keys for retries, a basis on
  reads, strict raw parsing, and separated error codes. It also found the CLI
  stdout gap.
- **Design reviews of the first draft (2026-11-05, Claude and Codex subagents
  through kit).** They found real problems that this revision corrects:
  - replying to notifications, which the specification forbids;
  - whole-batch rejection instead of a response per entry;
  - read envelopes and a strict parser that do not exist publicly;
  - a mismatch with how `encode_error` nests the error object;
  - promised parallel reads that the single shared connection cannot provide;
  - an `open` that would create files;
  - a wire-output change for `basis`;
  - missing hazards around trust, framing, result size, unkeyed `Unknown`
    outcomes and backup lineage;
  - an "or" build trigger that contradicted its own rationale.

The design differs from the second opinion in one respect: it treats this as the
primary external interface rather than one more adapter. The reason is that
three consumers are already visible: the deferred Elixir integration,
per-command CLI open cost, and agent access through MCP. It stays deferred
until those library decisions settle.

## Revision of 2026-11-23

Made when the user authorized building, to bring the design up to the library
as it stands and to settle what the first version left open.

- **Added `verify`, and `validation` on `open`,** for the structural open and
  `verify` that plan 12.1 added.
- **`at` also takes the compact token string.**
- **Unknown and duplicate members of `params` are refused** with -32602.
- **Ids are echoed byte for byte,** and an id of any kind but string, number
  or null is refused.
- **No notification is executed,** not only the order-dependent ones.
- **Blank lines are skipped;** oversized lines are discarded while read.
- **One thread.** The first version left the threading open.
- **`--require-idempotency-key`** is built, not only suggested.
- **The read decoders are in the wire module,** and the envelope parser keeps
  member text instead of rebuilding it.
- **The `Unknown` recovery test is left to the library's fault suite.**
- **The CLI's outcome when its report cannot be written is still undecided.**

