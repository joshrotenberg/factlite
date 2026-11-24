# Review 0058: MCP front end

Branch `feat/mcp-front-end`, PR 24. Plan item 14.3. Adds `factlite-mcp` to
`crates/factlite-rpc` on the dispatch merged in PR 23. No library code
changed. No human reviewer ran.

## Method

- **Self-review** of the complete diff, with 32 hand mutations of the front
  end.
- **Claude subagent,** on `6d4ed67`, in a detached worktree. It read the
  specification pages, ran the binary against stores made with the CLI, and
  committed from another process between pages.
- **Codex subagent,** through delega, high effort, on `6d4ed67`, in a
  detached worktree with sandboxed commands and no network. It ran the
  tests, drove the binary, applied its own regressions in scratch space, and
  marked where it could not check the specification offline.

## Self-review findings

- **Three tests were wrong on first run:** a snapshot passed as JSON text
  instead of a token, the expected code for an undecodable token, and a
  paging test that read every tool's whole result at one early snapshot
  while the head moved between tools. Corrected.
- **Five of 32 mutations survived:** read-only stores opened writable,
  structural validation ignored, `transact` listed with no writable store,
  `store` required with one store bound, and store names unchecked inside
  the library type. Each now fails a test. A sixth survivor changed `now`
  into an equal number and is not a real change.

## Claude findings

1. **Read-only bindings create SQLite's sidecar files, and a store in a
   read-only directory cannot be served.** Documented in the design, the
   guide and the decisions.
2. **`--page-bytes` was described as counting from the second item.**
   Corrected; the code counts every item and keeps one when it alone is
   larger.
3. **The read-only test never reached the connection.** Adopted before this
   review was read, with the mutation survivors: a damaged store must fail a
   full open and serve with structural validation, which only a read-only
   connection accepts.
4. **A pager returning one item per page passed the byte test.** Adopted:
   each page must leave no room for the next page's first item.
5. **No write was sent as a notification.** Adopted: a `transact`
   notification commits nothing.
6. **An empty key counted as a key.** Adopted for both servers: an empty key
   is refused where a key is required, and the schema says `minLength: 1`.
7. **Fractional ids were accepted.** Adopted: ids are strings or integers.
8. **`_meta` messages misled, and `clientCapabilities` of any type passed.**
   Adopted.
9. **The schemas call `1.0` an integer and the tools refuse it.** Documented
   in the descriptions; the wire contract's integers are not changed.
10. **An offset past the end is clamped in the reply.** Documented.

## Codex findings

1. **`transact` was marked not destructive,** though a retraction removes a
   fact from the current view. Adopted: `destructiveHint` is true for
   `transact`.
2. **Batches were refused under the 2025-03-26 revision, which allows
   them.** Adopted: batches are answered entry by entry in every era.
3. **`initialize` checked only `protocolVersion`.** Adopted: `capabilities`
   and `clientInfo` must be objects. An `initialize` carrying a 2026-07-28
   `_meta` is still answered in the handshake form; not changed.
4. **`clientCapabilities` was checked for presence only.** Adopted, as Claude
   finding 8.
5. **A repeated argument hid an unknown tool,** and `cursor: false` was
   ignored. Adopted: the tool name is resolved first, and any cursor is
   refused because no list is paged.
6. **Store names sat in the URI authority,** which a client may lowercase, so
   `Upper` and `upper` could collide. Adopted: `factlite://stores/NAME/schema`.
7. **The schemas admitted inputs the decoders refuse.** Adopted: tagged
   values, change entities, terms, patterns and predicates, `offset` needing
   `at`, and the operator's metadata keys. The integer `1.0` difference is
   documented, as Claude finding 9.
8. **The instructions said every read returns `basis`.** Corrected: every
   read except `head`.
9. **Test gaps:** read-only bindings, write notifications, paging of
   `schema`, and a failing tool call in the stateless form. All adopted. The
   test's validator now counts `1.0` as an integer, as JSON Schema does.
10. **Docs:** sidecar files and "always at least one item". Corrected.

## Mutations

Single changes to the front end's source, applied by hand by a script that
is not in the repository, each followed by the crate's MCP and stdio tests.

- **First run, 32 mutations:** 25 caught, 1 skipped, 6 survived. Five
  survivors were real and are closed (see self-review); the sixth changed
  `now` into an equal number.
- **Final run, 40 mutations:** the 32 retargeted to the revised code, and
  eight for the review changes: fractional ids, dropped batch entries,
  `transact` marked not destructive, cursors ignored, `initialize` not
  checking its objects, empty keys, the store name in the URI authority, and
  two schema rules. 39 are caught; the remaining one is the `now` mutation.

## Validation

`REQUIRE_BB=1 ./scripts/check.sh` passes locally on the final commit with
306 tests. GitHub Actions runs the same script on the PR head before merge.

## Limitations

- The 2026-07-28 form is tested only against this repository's scripted
  client. The specification facts were read through a subagent from the
  specification site on 2026-11-23; the Codex review had no network.
- Each page recomputes its read.
- Query rows tile across pages only under a total order.
- JSON Schema's integer `1.0` is refused by the tools.
