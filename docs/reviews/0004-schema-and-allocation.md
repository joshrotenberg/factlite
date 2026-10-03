# Review: item 1.2, schema and allocation

- **Branch:** `core/schema-and-allocation`
- **Reviewer:** Codex self-review.

Reviewed source, public API, unit/property/integration tests, and all staged
changes. Definitions have private immutable fields and validated cardinality/
uniqueness combinations. Historical schema/allocation filters use creation
revisions and reject future bounds. Entity membership checks both store binding
and actual allocation. Staging clones before mutation, so late failures cannot
modify source catalog state or expose committed IDs.

Added explicit regressions for a fabricated same-store ID and a schema failure
after a new definition was staged. Allocation exhaustion accepts the last u64
identity then fails without wrapping. Temporary dead-code expectation is limited
to this private catalog's pre-integration stage and documented for removal.

`scripts/check.sh` passed without warnings: eighteen tests including nine
1,024-case properties, plus formatting, Clippy, and docs. Public integration
checks cover validated definitions; durable membership scenarios need the store.
No unresolved findings. Catalog and history are in-memory semantics at this
stage; no SQLite durability claim is made.
