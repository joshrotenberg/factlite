# Using the Rust library

Run the complete, asserted workflow from the repository root:

```sh
cargo run --locked -p factlite --example task_workflow
```

[The source](../crates/factlite/examples/task_workflow.rs) uses a fresh temporary
directory, cleans up after itself and prints one versioned JSON query result.
It runs in `scripts/check.sh`. `cargo doc --open -p factlite --no-deps` opens the
API reference, including a tested minimal retry example.

The crate is currently unpublished. For an application beside this checkout,
add this dependency to its `Cargo.toml`:

```toml
[dependencies]
factlite = { path = "../factlite/crates/factlite" }
```

Adjust the relative path to your layout. Use the pinned Rust toolchain specified
in the checkout; older compiler compatibility has not been established.

## Create and update

`Store::open(path)` creates or validates a durable file. Attribute names are
validated `AttrName`s; definitions specify exact type, cardinality and uniqueness.
Definitions are immutable. Identical redeclarations are safe; changing a
definition conflicts. Use explicit `Value` variants: an integer and a float are
different values. `Value::f64` rejects nonfinite numbers and normalizes zero.

`Tx::builder()` owns its inputs. Declare `TempId` labels with `new_entity`, then
use those labels in assertions and `ValueTerm::TempRef` for references. Forward
references and cycles resolve together. `build()` checks request structure;
`commit()` checks current state and commits atomically. Read durable IDs from
`TxReport::tempids`. Save IDs and the original request if you need to retry.
`EntityTerm::Lookup(attr, value)` and `ValueTerm::LookupRef(attr, value)` name
the entity that owns a `unique_one` value before the transaction. They never
see values the same request asserts, never create entities (no match fails
`UnknownEntity` with `ErrorContext::Lookup`), and enter retry identity as the
lookup, not the entity it found. Expectations take `EntityId` only.

For a one-valued attribute, asserting the new value replaces the old value.
For a many-valued attribute, assertions/retractions operate on a set. Batch
operations describe a final state, so contradictory assertions/retractions of
the same fact conflict. Unique ownership transfers can occur atomically.

Use `expect(entity, attribute, values)` for an exact pre-state set condition;
an empty set means absence. Use `expect_head(snapshot)` when any intervening
commit, including a no-change commit, must invalidate the request.

## Permanent values

Every committed value, metadata entry, temporary label and idempotency key is
kept for the life of the store and copied into later backups; a keyed request
also keeps its expectation and lookup values. Retraction ends a fact's interval
but does not remove the value. Store data that may have to be deleted in an
external record and keep a reference, or store it as `Value::Bytes` encrypted
under a key held outside Factlite. Encrypted values compare, enforce uniqueness
and query as ciphertext, and a keyed retry must resend the original ciphertext.
See [permanence and sensitive data](permanence.md).

## Read one basis

`store.db()` captures head once. `store.db_at(&snapshot)` reopens a retained
basis. Each `Db` owns access and survives dropping the original `Store`; results
are owned. A `Db` never refreshes itself after a write. Capture a new one when
you want current data.

`pull` is flat: references remain `Value::Ref`. Traverse explicitly with the
same `Db` to preserve consistency. A known empty entity returns `Some(Entity)`;
an unknown entity returns `None`. Missing attributes are omitted; requesting an
attribute unknown at that historical basis is an error.

Queries use `Query`, fixed-attribute `Pattern`s and explicit typed `Params`.
Shared variables join entity positions with reference values. Projection is
distinct; ordering refers to selected fields, and limits apply afterward.
Supply a total order (the example uses title, then task ID) for predictable
limited results. Every declared parameter must be supplied, even for limit zero.
`Query::negations` holds `Negation` groups: `join: None` is `not`, and
`join: Some(vars)` is `not-join` with group-local variables. A binding survives
only if no group holds for it at the view's revision.

`AggregateFn::Avg` returns F64; an I64 mean rounds the exact quotient once.
`Query::having` filters aggregated rows before ordering and the limit; its
variables name result columns (selected variables and aggregate names).

`history(filter, after)` returns effective fact events in `(after, basis]`.
`transactions(after)` includes full reports for no-change commits too. Attribute
definitions, allocations and metadata remain available through retained reports.

## Retry and recover

Use a unique application operation key in `idempotency_key`. Retrying the same
typed request returns the original complete report and IDs before checking
current conditions. Preserve operation order, labels, metadata and head fences;
changing the request under an already committed key yields `IdempotencyConflict`.
A failed transaction reserves no key. A fresh valid request always advances head.

Match `Error::code()` and inspect `context()` for application decisions. Codes
are nonexhaustive; retain a catch-all arm. `PreconditionFailed` and `HeadMismatch`
require deciding again against a fresh view. Do not silently reinterpret a failed
condition. `commit_outcome()` distinguishes confirmed rollback from an unknown
outcome; after an uncertain commit or lost response, retry the original keyed
request to recover its report. See [CLI errors](cli.md) for JSON equivalents.

Save `wire::encode_snapshot` output and recover it with `wire::decode_snapshot`.
Tokens include store, lineage and revision; a revision alone is insufficient.
`Store::backup` produces a validated standalone copy, including committed WAL
contents. `Db::open_read_only_at` reads that copy at an original token without
creating a writer. See [backup and lineage rules](backup.md) before continuing a
restored writer: the original must be retired, and divergent clones are unsupported.
