# Decision brief: history retention and excision (roadmap item 6)

Status: decided 2026-11-06 (see Decision). Options A and B are documented in
`docs/permanence.md` (plan item 12.4).

## The question

Factlite keeps every committed fact forever. Should there be a way to remove
data that must not be retained, such as a secret written by mistake or
personal data under a deletion request? If so, what does removal mean for
history, snapshot tokens, retries and backups?

## What the design and code commit to

The design says: "retain all committed history in v1. No pruning or compaction
that discards semantic history." Several guarantees rest on that:

- **Snapshot tokens.** A token names a revision, and every read at it returns
  the same answer forever, including after reopen and from backups
  (`docs/backup.md`).
- **Integrity.** Every open replays and validates the retained log, and checks
  the datom intervals and indexed state against it (`store.rs`, `log.rs`,
  `index.rs`).
- **Retries.** A keyed retry compares stored canonical request bytes and
  returns the stored original report (`log::retry_report`).

A single stored value is copied into several places:

- `fact_events.value`, the authoritative events;
- `datoms.value`, the derived intervals, and its copies in the `datoms_open`
  and `datoms_avet` indexes;
- the encoded report in `transactions.report`, as effective deltas and caller
  metadata;
- `transactions.canonical_request`, for keyed requests. It also holds values
  that never became facts: expectation values and ineffective operations;
- outside the live tables: SQLite free pages and the WAL, until vacuumed and
  checkpointed; and every backup made earlier.

Factlite sets no `secure_delete` pragma and never vacuums.

No consumer has asked for this. The clajent kit stores run metadata (keys,
recipes, models, paths, costs), not prompt or file contents.

## Options

### A. No excision; document the rule

Keep history immutable. Document that sensitive data should not be written as
fact values: store a reference to an external record instead, which the
application can delete.

- **Guarantees:** all kept.
- **Cost:** none.
- **Limitation:** a mistake cannot be undone except by restoring a backup made
  before it and replaying later work, which `docs/backup.md` does not support
  as a lineage operation.

### B. Application-level encryption (crypto-shredding)

Applications store sensitive values as `bytes`, encrypted with a per-subject
key held outside Factlite. Deleting the key makes every copy unreadable,
including in backups, the WAL and free pages.

- **Guarantees:** all kept. Factlite sees opaque bytes, and replay needs no key.
- **Equality:** randomized encryption makes equal plaintexts unequal, so
  duplicate assertions, retractions, expectations and uniqueness all operate on
  ciphertext. Deterministic encryption gives plaintext equality only within one
  key and context, so per-subject keys cannot enforce plaintext uniqueness
  across subjects. Encrypted values cannot be queried or ordered by content.
- **Retries:** a keyed retry must resend the original encrypted request.
  Re-encrypting after a lost response produces different canonical bytes and
  fails `IdempotencyConflict`.
- **Scope:** it is an application pattern, not an unconditional guarantee.
  Shredding works only if every copy of the key is destroyed, no plaintext went
  into metadata or exports, and the value was never committed in plaintext.
- **Factlite work:** none required. A helper or an encrypted value type could
  come later.

### C. Rewrite into a new lineage

A tool reads a store and writes a new store with a new lineage identity. It
replays every transaction except the excised facts, then retires the old file.

- **Guarantees:**
  - Each lineage's history stays immutable, and nothing in place changes
    meaning.
  - Old tokens fail `InvalidSnapshot` against the new file, as they would
    against any other store.
  - Every invariant is checked by normal validation of the new file.
- **Costs:**
  - Filtered replay is not enough. Replay re-evaluates keyed requests and
    requires exact report equality, and changed history can invalidate
    expectations or turn changes into no-ops. The tool must define revision
    mapping, rewritten `expected_head` tokens, what happens to reports and
    keys, reference remapping, and allocation preservation.
  - The design deferred "any relineaging operation until it defines how entity
    identities and persisted reports behave". Entity IDs embed the store
    identity, so they would change, or the tool would keep the store identity
    and change only the lineage.
  - Removing a fact can make later transactions invalid or different: a
    uniqueness transfer, an expectation that checked it, or a retained keyed
    request whose evaluation now differs. The tool needs a rule for those
    cases, such as refusing, or rewriting their reports and dropping their
    retry evidence.
  - The old file and its backups still hold the data until destroyed.
- **Work:** large.

### D. In-place excision

A new operation records an excision at revision X. Rows that hold the value are
rewritten, before and after X, replacing it with a typed tombstone such as a
content hash. Replay, interval checks, reports and canonical requests learn to
accept tombstones. Retry identity of affected keyed requests would compare
hashes.

- **Guarantees:**
  - Tokens stay valid, but their reads change: excised facts disappear from
    every view.
  - Every validation path must treat tombstones consistently, including
    uniqueness and expectations evaluated over them.
  - A content hash keeps guessable secrets discoverable, and it does not fit
    the typed codec of every value type.
  - Hash-only retry evidence contradicts the design's requirement of exact
    canonical equality.
  - See "Physical deletion" below.
- **Work:** very large. It touches nearly every invariant this project tests.

### Other alternatives

- **Sanitized export:** export the current state, without history, into a
  fresh store.
- **Whole-store destruction.**

Both are simpler than C, and both lose history.

## Physical deletion

Any option that rewrites data must also remove the old bytes.

- **Database remnants:** `secure_delete` set before the rewrite, or a `VACUUM`
  afterwards, removes them from the database file; both are not required.
- **WAL:** an ordinary checkpoint can leave bytes in the WAL. A truncating
  checkpoint must succeed, which active readers can prevent.
- **Outside the file:** none of this erases filesystem snapshots, SSD
  remnants, or backups made earlier. `Store::backup` copies complete history
  without sanitizing it, and deleting the original file is not erasure.

## Recommendation

**A and B now; C only if a real deletion requirement appears.**

- Adopt A: state in the README and the Rust and pod guides that fact values are
  permanent, and that sensitive data belongs in an external record or encrypted
  bytes.
- Document B as an application pattern, with its equality, uniqueness and retry
  limits and the conditions for shredding to work.
- Rule out D: it changes the meaning of existing tokens and retained evidence,
  which the project's guarantees are built on.
- If a deletion requirement arrives, design C, starting with the relineage
  questions the original design left open.

## Second opinion

Codex reviewed the first draft (2026-11-06, through kit). It agreed with A and
B now, deferring C and rejecting D, and found the account of storage and
integrity substantially correct.

Its corrections, checked against the code and adopted:

- **Crypto-shredding is an application pattern with limits.** Ciphertext
  equality, per-subject uniqueness, and retries that must resend the original
  encrypted request.
- **Physical deletion was imprecise.** `secure_delete` or `VACUUM`, not both;
  a truncating checkpoint, which readers can block; and no erasure of
  snapshots, SSDs or earlier backups.
- **The index copies and the non-fact values in canonical requests were
  missing.**
- **C needs revision, token, key, reference and allocation rules, not just
  filtered replay.**
- **Sanitized export and whole-store destruction were missing alternatives.**
- **D's hashes keep guessable secrets findable and break exact retry
  equality.**

## Decision (user, 2026-11-06)

Adopted the recommendation:

- **Now:** options A and B, as documentation only (plan item 12.4).
- **Only if a real deletion requirement appears:** design option C.
- **Ruled out:** option D.
