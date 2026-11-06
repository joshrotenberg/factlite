# JSON-RPC interface: design, deferred

Status: designed 2026-11-05, not authorized to build. This document fixes the
shape of Factlite's process-boundary interface so library work can account for
it. Nothing here is implemented. `AGENTS.md` still excludes a server until the
user authorizes building this.

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

Like the CLI and pod, it is a thin adapter over the library and the strict wire
contract in `crates/factlite/src/wire/`.

## When to build

Build once the parts this interface exposes stop moving:

- open-time integrity is decided (roadmap item 3);
- lookup refs in transactions are decided (item 4).

A concrete consumer being ready (Elixir, or agents through MCP) also qualifies.
Items 5 and later only add methods or params, and need not block it.

## Transport and lifecycle

- **Stdio first:** newline-delimited JSON-RPC messages on stdin and stdout,
  diagnostics on stderr. This matches how editors, MCP clients and Elixir ports
  launch subprocesses.
- **Optional later:** a local Unix socket for several clients of one process.
  No TCP; the design excludes network promises.
- **Stores:** one process serves one or more stores opened by path. A store stays
  open for the process lifetime, so opening and validating the log happens once.
- **Ending:** the process ends cleanly at end of input or on `shutdown`. In-flight
  commits finish or roll back; none is abandoned midway.

## Methods

Params are always an object. Each method's params embed the existing wire
envelope unchanged, and results are the existing wire outputs plus the read
basis (see below). A store argument is the handle returned by `open`.

| Method | Params | Result |
| --- | --- | --- |
| `open` | `path`, `read_only?`, `busy_timeout_ms?` | `{store, head}` |
| `close` | `store` | `{}` |
| `head` | `store` | snapshot |
| `transact` | `store`, `tx` (wire request) | wire report |
| `schema` | `store`, `at?` | wire schema plus `basis` |
| `facts` | `store`, `filter?`, `at?` | wire facts plus `basis` |
| `pull` | `store`, `entity`, `attrs`, `at?` | wire entity plus `basis` |
| `lookup` | `store`, `attr`, `value`, `at?` | wire lookup plus `basis` |
| `history` | `store`, `filter?`, `after?`, `at?` | wire events plus `basis` |
| `transactions` | `store`, `after?`, `at?` | wire records plus `basis` |
| `query` | `store`, `query` (wire envelope), `at?` | wire result plus `basis` |
| `backup` | `store`, `to` | snapshot of the copy |
| `shutdown` | none | `{}` |

`at` is a full snapshot token. A method with a typed request has no shorthand
form: `transact` takes exactly one complete request.

## Rules

**Atomicity.** JSON-RPC batches may execute independently and in any order, so
they are never transactions. Atomicity comes only from one `transact` call with
one complete request. Batches of reads are allowed. A batch containing
`transact` is rejected, so callers cannot mistake it for an atomic unit.

**Writes always respond.** A `transact` sent as a notification (no `id`) is
rejected without committing. The `id` only correlates a response with its
request; it is not an idempotency key. Clients that may retry should send an
`idempotency_key`. After a timeout or disconnect, they retry the same request
with the same key, which returns the original report if it committed. An
unkeyed retry is a new request and can create a second revision.

**Reads report their basis.** A read without `at` captures head once, for that
call only. Two such reads can see different revisions. Every read result
therefore includes the snapshot it read (`basis`). Coordinated reads pass that
snapshot as `at`. A snapshot is not a write fence; writes still use `expect` or
`expected_head`.

**Strict parsing.** Params are parsed from the raw message text by the wire
module's strict parser, so duplicate keys, unknown fields and malformed tagged
values fail exactly as in the CLI. Factlite's request `version` stays in the
envelope; `"jsonrpc": "2.0"` does not version Factlite semantics.

**Concurrency.** Requests from one connection are handled in order. Reads may run
in parallel, each in its own SQLite snapshot. Commits serialize on the store, as
they do across processes. Parallel dispatch adds no write throughput.

## Errors

- **Protocol failures** use the standard codes: -32700 parse error, -32600
  invalid request (including a notification or batch containing `transact`),
  -32601 unknown method, -32602 params that are not an object or lack a
  required member.
- **Factlite errors** use code -32000. `error.message` is the library message.
  `error.data` is the existing wire error object: stable code, structured
  context, and `commit_outcome` (`RolledBack` or `Unknown`) for commit failures.
  Clients branch on `error.data.code`, never on the message or the numeric
  JSON-RPC code.
- **Busy** is a Factlite error (`data.code` `Busy`), retryable as elsewhere.

## Library work this design depends on

These are worth doing regardless, and should land before or with the crate:

- Read results carry their basis: `wire::encode_facts`, `encode_query_result`
  and the other read encoders gain the snapshot. This also helps the CLI and
  pod.
- The CLI can commit and then fail while writing the report to stdout. The CLI
  should exit with a distinct code saying the commit outcome is known. The RPC
  server has the same hazard on a broken pipe; a keyed retry recovers the report
  in both cases.
- A wire envelope for `lookup` input, matching `Db::lookup`.

## MCP

MCP is JSON-RPC 2.0. An MCP server would expose the same dispatch as MCP tools
(`transact`, `query`, `pull` and so on, with JSON schemas generated from the wire
contract) on the same transport. It is a second front end on `factlite-rpc`'s
dispatch, not a separate implementation, and is decided separately.

## Testing when built

- **Parity:** every method gives results identical to the library and CLI for
  the same wire input, including all errors.
- **Hazards:** a batch containing `transact` and a notification `transact` are
  both rejected without committing; a keyed retry after a dropped connection
  returns the original report; duplicate keys inside params are rejected.
- **Contention:** several server processes commit to one file, as in the pod's
  contention test.
- **Transport:** `assert_cmd` scenarios over stdio.

## Second opinion

A Codex review of the idea (2026-11-05, through kit) recommended against making
JSON-RPC the core interface and suggested an optional adapter built only on
demand. Its conditions are adopted above: no batch atomicity, no write
notifications, idempotency keys for retries, a basis on reads, strict raw
parsing, and separated error codes. It also found the CLI stdout gap.

The design differs in one respect: it treats this as the primary external
interface rather than one more adapter. The reason is that three consumers are
already visible: the deferred Elixir integration, per-command CLI open cost,
and agent access through MCP. It stays deferred, as Codex advised, until those
library decisions settle or a consumer is ready.
