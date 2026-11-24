# Review 0057: wire read decoders and the JSON-RPC server

Branch `feat/rpc-server`, PR 23. Plan items 14.1 and 14.2. Adds public read
decoders to the library's wire module and a new crate, `crates/factlite-rpc`.
No storage semantics changed. No human reviewer ran.

## Method

- **Self-review** of the complete diff, with hand mutations of the new crate
  and repeated runs of the process tests.
- **Claude subagent,** read-only on the repository, on `4539b5b`. It ran the
  crate's tests, drove the debug and release binaries with adversarial input,
  and compared 31 read inputs with the CLI binary by hand.
- **Codex subagent,** through delega, high effort, on `4539b5b`, in a
  separate detached worktree with sandboxed commands. A first attempt in
  read-only mode could not read files and produced no review. It ran the
  workspace tests offline, reproduced each finding, and applied its own
  mutations in scratch space.

## Self-review findings

- **Three mutations survived the first run of 38.**
  - Ignoring the busy timeout passed, because the test's 700 ms bound sat
    below the library's two-second default. The test now uses 2,600 ms.
  - Dropping the flush passed, because Rust's stdout flushes at each newline.
    A unit test now serves through a writer that shows bytes only when
    flushed.
  - A digit check before parsing an unsigned integer was redundant: raw JSON
    number text that parses as `u64` is digits. Removed.
- **The query fixture in a store test used the wrong pattern form.** It
  failed at once; corrected to the `[entity, attr, value]` form.

## Claude findings

1. **The repeated-key check was quadratic.** One 3.5 MB line, inside the
   default limit, held the single-threaded server for 51 s. Adopted: a hash
   set, and a unit test with 200,000 keys.
2. **The escaped-duplicate cases were identical to the plain ones.** The
   escapes were lost when the files were written. Adopted: both now carry a
   `\u` escape.
3. **Malformed wire members were checked by code only.** Adopted: a test
   sends 23 malformed members and compares each error object with the
   library decoder's own error for the same text. The looser test is
   removed.
4. **Read-only handles were outside the parity property.** Adopted: every
   read in the property runs on a writable and a read-only handle.
5. **Exit code 5 was untested.** Adopted: a server whose stdout has no reader
   exits 5.
6. **No test runs the CLI binary.** Not adopted: a binary of another package
   is not built for this crate's tests. The guide now says results come from
   the library encoders the CLI uses, and that the CLI is not run.
7. **An empty file is initialized without `create`.** Documented; the CLI
   does the same.
8. **-32602 also covers values of the right kind but outside their form.**
   Documented.
9. **The design still said `close` waits for requests in flight.** Corrected.
10. **Wire error positions count from the member, not the line.** Not
    changed. Messages are not for branching, and the positions are the
    library decoders' own.
11. **A number out of range in a batch entry got -32700.** Adopted, with
    Codex finding 2.
12. **The existence check duplicates the CLI's, and races.** Documented, with
    Codex finding 5; not changed.

## Codex findings

1. **A repeated key hid a syntax error.** `{"id":1,"id":}` got -32600.
   Adopted: the whole line is checked as JSON before the top level is opened.
2. **A number too large for a machine type was a parse error.** `1e400` is
   JSON. Adopted: numbers are no longer converted, so it is a scalar (-32600),
   and in a batch an entry that cannot be read is -32600.
3. **`attrs` can name an attribute with a comma, which the CLI cannot.**
   Documented as a difference; the library allows such names.
4. **The missing-path message differed from the CLI's.** Adopted: the
   message is the operating system's, as in the CLI.
5. **`open` without `create` can create a store if the file is removed after
   the existence check.** Reproduced by Codex. The CLI has the same race.
   Not fixed: it needs an "existing store only" open in the library, which
   changes the library's API. Documented in the guide, the design and the
   decisions.
6. **The existence check hid the library's refusal of a writable structural
   open.** Adopted: the check is skipped for that case, and a test expects
   the library's `InvalidRequest`.
7. **The repeated-key check was quadratic.** As Claude finding 1.
8. **`--help` and `--version` panicked on a closed pipe.** Adopted: they exit
   5, tested.
9. **Successful commits had no independent oracle.** A mutation that dropped
   metadata from unkeyed requests passed the property, because it compared
   the report with the log of the same commit. Adopted: a backup of the store
   keeps its identity and allocation state; the library commits every request
   to it, and its reports and errors must equal the dispatcher's.
10. **No `UnsupportedFormat` case; `Busy` checked by code only.** Adopted:
    both compared with the library's error object.
11. **Structural opens, `verify` and a zero timeout could regress unseen.**
    Adopted: a damaged report makes a full open fail, a structural open
    succeed and `verify` fail, each compared with the library; a zero timeout
    must fail within 1.5 s.
12. **A re-encoded string id passed.** Adopted: an id with a ` ` escape
    must come back as sent.
13. **Partial writes, bounded discarding and a lost committed response were
    untested.** Partly adopted: the flush test's writer takes seven bytes at
    a time, and a test kills the server only after the file shows its commit,
    then retries. That an oversized line is not held in memory is not tested.
14. **The guide omitted observable cases.** Adopted: batch entries that are
    not objects, an empty batch, a batch of notifications, `shutdown` without
    `params`, blank lines over the limit, the timeout's maximum, SQLite's
    sidecar files, and the read-only missing-path error.
15. **Status and roadmap disagreed with the branch.** The status entry was
    written after the reviewed commit; the roadmap wording is corrected.
16. **Trailing blank lines in two documents.** Corrected.

## Mutations

Single changes to the crate's source, applied by hand by a script that is not
in the repository, each followed by the crate's tests.

- **First run, 38 mutations:** 35 caught. The three survivors are listed
  under self-review and were closed.
- **Final run, 46 mutations:** the 38 again, adjusted to the code as it now
  reads, and eight more taken from the reviews: a single `write` that ignores
  its count, re-encoded string ids, metadata dropped from unkeyed requests,
  structural validation replaced by full on read-only opens, `verify`
  returning the head, a zero timeout ignored, repeats checked before syntax,
  and the missing-path message prefixed. All 45 that still apply are caught.
  The redundant digit check no longer exists.

## Validation

`REQUIRE_BB=1 ./scripts/check.sh` passes locally on the final commit with
291 tests. The rpc process tests passed repeated runs. GitHub
Actions runs the same script on the PR head before merge.

## Limitations

- The tests compare with the library, not with the CLI binary.
- A commit whose outcome is `Unknown` is not exercised through the server;
  the library's fault suite covers the keyed retry.
- That an oversized line is dropped without being held is shown by reading,
  not by measuring memory.
- The existence check races a removal of the file, as the CLI's does.
- Process tests ran on one macOS machine locally; Linux coverage comes from
  CI.
