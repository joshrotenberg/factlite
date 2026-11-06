# 11.1 Open options and read-only pod connections: self-review

Reviewed the complete diff. This is a self-review. No independent review or
remote CI ran.

## Change

`StoreOptions` (busy timeout) is threaded through writable and read-only
opens. `Db::at_head` and `Db::at` give new views over an existing connection.
The pod registry holds `Conn::Writer(Store)` or `Conn::Reader(Db)`, and `open`
parses `{:read-only :busy-timeout-ms}` strictly.

Self-review checked these points:

- Defaults are unchanged: two seconds everywhere, including backups, which
  keep their own fixed timeout.
- Timeout validation rejects values rusqlite would truncate or overflow.
- `at_head` on a read-only view runs the same deferred head read as
  `Store::head`. `at` validates identity and bounds like `db_at`.
- A read-only pod connection cannot reach `Store::commit`: the pod refuses
  before translation, and the underlying connection is read-only regardless.

## Evidence

- Library tests: a 150 ms timeout fails `Busy` after at least 150 ms and well
  under 2 s; zero fails at once; sub-millisecond and too-large timeouts are
  rejected; a read-only view stays pinned while `at_head` sees later commits,
  `at` returns history, and a future snapshot fails.
- babashka (both transports, 67 checks): read-only head follows the writer,
  history through `:at`, commits refused, later commits visible, zero and
  invalid timeouts, unknown options, and read-only open of a missing file.

Validation: `scripts/check.sh` passed on macOS 27.2, Rust 1.99.1, with 169
tests, the Rust workflow and warning-free rustdoc.

## Limitations

- The CLI does not expose the busy timeout.
- A read-only open still replays the full log once.
