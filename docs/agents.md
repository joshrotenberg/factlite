# Guide for agents that write to Factlite

For authors of agents and agent harnesses that keep notes, decisions, run
records or work items in a Factlite store. Plan item 13.2.

The conventions here are exercised by a runnable example, except where a
section says otherwise:
[`examples/agent_session.rs`](../crates/factlite/examples/agent_session.rs).

```sh
cargo run -p factlite --example agent_session
```

Background and what is not built: [agent-native exploration](design/agent-native.md).

## What a store gives an agent

- **Reads that agree with each other.** Take a snapshot once and read
  everything at it. Other agents' commits do not change what you see.
- **Nothing is overwritten.** Every earlier state can be read as it stood,
  and each fact's history lists its values in order.
- **Writes that refuse to clobber.** A commit can require the exact values it
  was planned against.
- **Retries that do not duplicate.** A repeated request with the same key
  returns the first commit's report.

It does not give text or vector search, a server, or any way to delete what
was written.

## Before writing anything: permanence

Factlite keeps every committed value for the life of the store. Retracting a
fact ends it; the value stays readable in history, in the file and in every
backup. The full list of what is kept is in [permanence](permanence.md). For
an agent the rule is short.

**Never write:**

- secrets, credentials, tokens or keys;
- raw tool output, file contents or command output;
- raw conversation text;
- personal data that someone may later ask to have deleted.

An agent that stores what it reads will meet all of these.

**Write instead** a reference to material kept somewhere that can delete it,
and a short summary you are willing to keep forever. In the example a note
holds `note/source`, a locator, and never the material.

Three more things are permanent and easy to miss:

- **Transaction metadata, temporary entity labels and idempotency keys.** Do
  not build them from user text.
- **Values in refused requests are not stored,** but errors repeat them. A
  failed expectation reports the expected and actual values, and they land
  in whatever logs the error.
- **A hash is not a reference.** A hash of an email address can be matched
  against candidate addresses. Use a random identifier.

## One store file per scope

A store file is the only unit that can be forgotten: delete the file and its
backups. So choose what a file holds by what you may need to drop together.
One per project, one per session, one per queue are all reasonable.

- **References do not cross files.** Refer across files by a stable string or
  keyword in a unique attribute, and look it up.
- **Each file is opened separately.** A writable open validates the whole log,
  measured at roughly 15 to 50 microseconds per revision
  ([measurements](measurements.md)). A read-only open can skip that with
  structural validation.
- **Several agent processes can write one file.** Commits are serialized by
  SQLite's lock on one host.

## Conventions

### Stamp every commit

Factlite records commit order. It does not record who wrote a commit or when.
Put the agent, the session and the time in transaction metadata on every
commit. `transactions` returns them.

The exception is shared setup that any process may repeat, such as declaring
the schema. For the repeat to be exact, every caller must send the same
request, so it carries a fixed key and no author or time. The example's
schema commit is written this way.

Metadata cannot be matched by `query`. If you need to find things by author or
time with a query, write those as facts as well, as the example does with
`note/at`.

### Supply the time

Any ordering by recency needs an `inst` the writer stored. "The ten most
recent notes" is a query ordered by `note/at` with a limit.

### Key every write by its intent

Give each write an idempotency key made from the session and the step, such
as `session-41/decide/cite-by-session`. A harness that retries the step sends
the same key and gets the first report back, with no second commit.

The retried request must be identical, including its timestamp. Take the time
once per step, not once per attempt. The same key with different contents is
refused with `IdempotencyConflict`.

Build keys so that two different steps cannot produce the same string. If a
session or step name can contain the separator, encode the parts or use fixed
lengths.

### Find or create, never assume

Unique values do not upsert. To get an entity by a unique name: look it up,
create it if absent, and if the create fails with `UniqueViolation`, look it
up again, because another agent created it in between. The example's `topic`
function does this, and two agents starting from the same empty view end up
with one topic.

Do not carry temporary labels or entity IDs across stores. Carry the unique
name.

### Keep one value in force with an expectation

When there must be exactly one current decision, owner or status, hold it in
a single-valued attribute and make every change expect the value it replaces.
In the example `topic/decision` points at the note in force. A change expects
the prior decision, or expects none for the first.

Two agents that both planned against the same view cannot both commit. The
second is refused with `PreconditionFailed`, commits nothing, reads again and
decides whether its change still applies. There is never a second decision in
force.

The chain of decisions needs no extra bookkeeping. The history of
`topic/decision` lists every decision in order, and a read at an earlier
snapshot returns the one in force then. A store without history has to record
supersession as explicit links, and nothing stops two writers from each
adding a successor.

### Load context at one snapshot

At the start of a turn take the head once and make every read at that
snapshot. The reads then describe one state, however long they take and
whoever else is writing. Keep the snapshot: it is the basis for the next two
conventions.

Bound what you load. Results are complete, with no cap and no continuation,
so use a query `limit`.

### Ask what changed since

Before acting on a view that may be stale, read `transactions` after your
snapshot's revision. Each record carries its metadata, so the answer says who
wrote and when, and its report says what changed. `history` after a revision
gives the same for one entity or attribute.

### Hand off a token, not a summary

A snapshot encodes to a short string naming the store, its lineage and a
revision. Pass it to the next agent. A read at that token returns what the
sender saw. The receiver then asks what changed since.

The example encodes a token, decodes it and reads at it through the same open
store. That a token still resolves after the store is reopened, and in a
backup that contains its revision, is shown by the library's
[revision read](../crates/factlite/tests/revision_reads.rs) and
[backup](../crates/factlite/tests/backups.rs) tests.

## Work items

Durable work queues, with leases and takeover of a dead worker's job, are in
the [queue guide](queues.md).

## Interfaces

The Rust API, the [CLI](cli.md) and the
[babashka pod](../crates/factlite-pod/README.md) share these semantics. The
example is Rust. There is no server and no MCP interface; the design for one
is in [`design/json-rpc.md`](design/json-rpc.md) and is not authorized to
build.
