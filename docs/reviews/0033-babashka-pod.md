# 10.1 Babashka pod: self-review

Reviewed the complete crate, test, script and documentation diff. This is a
self-review. No independent review or remote CI ran.

## Change

`factlite-pod` serves `pod.joshrotenberg.factlite` over transit+json with
`babashka-pod` 0.1.1. Connections live in a `Handles<Store>` registry; each
call clones the `Store` out, so the registry lock is held only briefly and
`Store` serializes its own connection. Inputs are translated into version-1
wire JSON and decoded by the library's wire decoders. Outputs are built from
library result types. Errors are `ex-info` with the wire error's code,
message, context and commit outcome.

Self-review checked these points:

- No validation is duplicated or weakened. Shape checks in the pod only
  decide how Clojure data maps to wire positions. Wire decoding still rejects
  unknown fields, bad versions, malformed tokens, nonfinite floats,
  temporary references outside changes and reference metadata.
- The value mapping is a bijection over the six types. Keywords are rejected
  rather than turned into strings, and longs and doubles keep distinct tags.
  Negative zero normalizes as in the library.
- Map keys that are user data (metadata, params, inputs, tempids, pulled
  attributes) are never case-converted. Only pod-defined keys use kebab-case.
  Keyword and string keys that collide after translation fail as duplicates.
- A pod request and the CLI JSON for the same request produce equal `Tx` values
  and identical canonical bytes, so mixing the two under one idempotency key
  behaves as one client.
- `:at` revision numbers become full tokens bound to the connection's store
  and lineage, then go through `Store::db_at`. Future revisions fail
  `InvalidSnapshot`, and a foreign store's snapshot map fails the same way.
- Handler panics and runtime errors are returned by `babashka-pod` as error
  replies, so the pod process keeps serving. This was observed for every error
  check in `test.bb`; panics were not injected deliberately.

## Evidence

- Rust: 8 translation tests, including one 1,024-case value round-trip
  property, rejection of 17 unsupported values, strict transaction, query and
  snapshot maps, error data, and pod/CLI canonical parity.
- babashka 1.13.229: `test.bb` passes 59 checks on both stdio and socket
  transports. They cover the design fixture (create, conditional update,
  historical pull by snapshot and by revision), exact retry and conflict,
  precondition/head/unique failures as data, no-change revisions, queries with
  inputs, predicates and limits at head and at A, history and transaction
  bounds, filters, historical schema, all six value types including i64
  bounds, NaN/infinity/BigInt/ratio/keyword rejection, unknown keys, arity,
  close and reopen.
- Contention: four pod processes (one on the socket transport) each commit 15
  keyed creations and 15 conditional counter increments against one file. Every
  creation lands exactly once, the counter equals 60, and the head revision is
  exactly 1 + 120, so no failed attempt committed. Five consecutive runs passed
  with 5 to 15 retries each. A manual run with 8 processes x 60 iterations passed
  with 183 retries.
- The README example ran under babashka with the documented outputs.
- Size: release `pod-factlite` on macOS arm64 is 3,361,936 bytes stripped and
  1,601,849 bytes as a zip.

Validation: `scripts/check.sh` passed on macOS 27.2, Rust 1.99.1. That covers
formatting, all-target Clippy with warnings denied, 158 tests (forty-four
1,024-case properties, one ignored manual profiler), the Rust workflow and
warning-free rustdoc. The babashka tests ran because `bb` is installed here.

## Limitations

- The CI workflow does not install babashka, so on CI the babashka tests would
  skip unless `REQUIRE_BB` is set. No remote CI has run at all.
- The pod is not packaged for the pod registry, and no Linux or Windows build
  has been tested.
- Read-only opens, backups and raw wire JSON are not exposed.
- Each `open` validates full retained history (item 9.1 limitation), so a pod
  should keep its connection rather than reopening per call.
- Pod runtime errors (closed connection, malformed bencode) carry a message
  but no Factlite error data.
