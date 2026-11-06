# Decision brief: lookup refs in transactions (roadmap item 4)

Status: decided 2026-11-06 (see Decision). Not yet implemented.

## The question

Reads already accept lookup refs: `Db::lookup`, and `[attr value]` in the pod's
`pull` and filters (item 11.2). Should transactions accept them too, so a
writer can address an entity by a unique value (`[:run/key "A/run-3"]`) without
reading its ID first? If so, when is a lookup resolved, how does it enter retry
identity, and what happens when it matches nothing?

## What the code does today

- **Entity terms.** A transaction names entities with `EntityTerm::Id` or
  `EntityTerm::Temp` (`crates/factlite/src/tx.rs`). Reference values use
  `ValueTerm::Value(Value::Ref)` or `ValueTerm::TempRef`. Expectations take a
  concrete `EntityId` only.
- **Retry identity.** It is the canonical encoding of the submitted request
  (`canonical.rs`). Entity terms are encoded with tag 0 (ID) or 1 (temporary
  label). `Store::commit` looks up a key and compares canonical bytes before
  any state is read. A replay returns the original report even when the
  request's expectations are stale (`store.rs`, `log::retry_report`).
- **Evaluation.** It reads pre-head state. Expectations inspect pre-head, and
  uniqueness is checked against final state, so an atomic transfer of a unique
  value is valid (`model.rs`).
- **Commit loading.** Commits load only the state a request touches
  (`index::model_for_request`, item 9.5). That would need the owners of each
  looked-up value.
- **A pattern that works without this feature.**
  1. Look the entity up (`Db::lookup`).
  2. Commit with an expectation that its unique attribute still has that value:
     `expect [id attr [value]]`.

  If the value moved in between, the commit fails `PreconditionFailed` instead
  of writing to the wrong entity. It costs a read and a second round trip.

## Options

### A. Lookup terms in the request, resolved at pre-head

Add `EntityTerm::Lookup(attr, value)` and `ValueTerm::LookupRef(attr, value)`.
Each is encoded canonically with a new tag (2), so retry identity compares the
lookup itself, not the entity it resolved to.

Evaluation resolves every lookup once, against the pre-state, before
normalization. Commit then runs it inside the write transaction after the
retry check, and replay re-evaluates retained keyed requests against each
historical pre-state, as it does today. With IDs in hand, evaluation proceeds
as today: an ID and a lookup naming the same entity collapse before the
conflict checks.

- **Retries:** an exact replay returns the original report even if the value
  has since moved to another entity. This matches how stale expectations
  already behave.
- **Consistency:** resolution at pre-head means a lookup cannot see a value the
  same request assigns or transfers. That keeps resolution independent of
  batch order. A transfer can still name the old owner by lookup.
- **Example:** if E1 owns key X, a request that changes E1's key to Y and
  assigns X to E2 resolves every `lookup(X)` to E1, and final-state uniqueness
  accepts the transfer.
- **Compatibility in changes and reference values:** those positions already
  carry tagged terms, so tag 2 appears only in new requests, and existing
  canonical bytes and reports are unchanged, as with keyword and inst in 11.5.
- **Compatibility in expectations:** expectation entities are encoded as bare
  ID text, with no term tag. Allowing lookups there changes the layout, so it
  needs a canonical request version 2. Version 2 would be written only for
  requests that contain an expectation lookup, while version 1 is still read
  for all retained requests. Otherwise existing retry identities would change.
- **Wire and pod:** the wire format gains `{"lookup": {"attr": ..., "value":
  ...}}` in those positions, and the pod gains `[attr value]` there.
- **Commit loading:** the commit loader (`model_for_request`) must load each
  lookup's current owner. It must also load that owner's full touched
  attribute sets, the membership of resolved references, and the existing
  owners of unique values asserted through resolved entities. Otherwise
  replacements could omit retractions, or uniqueness could miss a collision.
  The full, touched-state and replay parity tests must cover lookups.
- **Work:** medium to large. Request types and builder, structural validation,
  canonical encode and decode, wire, pod, resolution in evaluation, the commit
  loader, retry and replay tests, and the parity and stateful generators.

### A1. A without expectation lookups (staged)

As A, but expectations keep concrete IDs for now. This avoids the canonical
version change. Expectation lookups can follow later as version 2.

### B. Resolve before canonicalization

Adapters (the pod, a future server) turn lookups into IDs before building the
request.

- **Retries:** retry identity then depends on state. A retry after the value
  moved resolves to a different ID, and the same key now fails
  `IdempotencyConflict`, even though the caller resent the same intent.
- **Races:** the adapter's read and the commit are separate, so a racing writer
  can move the value between them. Unless the adapter also adds an
  expectation, the write lands on the wrong entity.
- **Work:** small, but it weakens the contract the design is built around.

### C. No transaction lookup refs

Document the lookup-then-expect pattern above, perhaps with a pod helper that
does both steps.

- **Integrity:** unchanged and race-safe.
- **Cost:** one extra read and round trip per write that starts from a key.
- **Work:** documentation, and an optional helper.

## The missing-entity question

In every option, a lookup that matches nothing should fail, including in a
retraction and in an expectation of absence (`[]`). Checks come in this order:

1. The attribute must be visible at pre-head (otherwise `UnknownAttribute`).
2. It must be `unique_one` (otherwise `InvalidRequest`).
3. The value must have its type (otherwise `TypeMismatch`).
4. Only then does an unmatched lookup fail, as `UnknownEntity` with a new error
   context naming the attribute and value.

Creating the entity instead would be an identity upsert, which the design
excludes: temporary labels always allocate, and `unique_one` "never merges
entities or performs identity upserts".

Catching the failure and then submitting a request with a temporary label is
not an atomic create-or-update. Another writer can claim the key in between,
and the second request then fails `UniqueViolation`. Callers must handle that.

**Caution on expectations by lookup:** an expectation through a lookup checks
whoever owns the value at pre-head, not the entity the caller saw earlier.
After the value moves from E1 to E2, an expectation on `lookup(X)` can pass on
E2. To fence a specific entity, callers keep using its ID plus an expectation,
or `expected_head`.

## Smaller choices within A

- **Expectations:** allow lookups as expectation entities. Recommended, since
  they are the main conditional-update path.
- **Reports:** a report shows resolved IDs only where facts changed. A no-op
  change or an expectation-only request leaves no record of which entity a
  lookup chose. Adding a `lookups` map to reports would change the stored
  report encoding (FLR1). Recommended: not initially. Callers who need the ID
  can use `Db::lookup` at `report.before`.
- **Queries:** lookup refs as query constants are separate, and unnecessary,
  since a query can join on the unique attribute.

## Recommendation

**A1 first, then A**: lookup terms in changes and reference values, with
their own canonical tags, resolved once against the pre-state inside
evaluation. A missing match fails `UnknownEntity`, with no upsert. Reports are
unchanged.

Add expectation lookups later as canonical request version 2, once A1 is in
use, so the version change is a separate, reviewed step. Until then, C's
lookup-then-expect pattern is the safe way to make a conditional write by key.

B is not recommended. It makes retry identity depend on state, and adds a race
unless the adapter also writes an expectation.

## Second opinion

Codex reviewed the first draft (2026-11-05, through kit). It supported A, and
agreed that resolving at pre-head after the retry check is sound, since
`Store::commit` serializes writers and checks retry evidence before reading
state. It also agreed that B's retry weakness and race are accurately
described, and that leaving reports unchanged is reasonable.

Its corrections, all checked against the code and adopted:

- Expectation entities are bare ID text in the canonical encoding, so
  expectation lookups need a canonical version, not just a new tag. This led
  to option A1 and the staged recommendation.
- Replay re-evaluates keyed requests, so resolution belongs in evaluation, not
  only in commit.
- The commit loader needs more than the lookup's owner.
- IDs and lookups naming one entity must collapse before the conflict checks.
- The missing-entity rule needs a validation order. The suggested
  create-on-failure path is not atomic.
- Expectations by lookup follow the current owner, which is a semantic trap
  worth stating.

## Decision (user, 2026-11-06)

- **Adopted:** option A1. Lookup terms in changes and reference values, with
  their own canonical tags, resolved inside evaluation against the pre-state.
  A missing match fails `UnknownEntity`, with no upsert. Reports are
  unchanged.
- **Later:** expectation lookups, as a separate item with canonical request
  version 2.
- **Plan item:** 12.3.
