# Permanence and sensitive data

Factlite keeps every committed value for the life of the store's lineage. There
is no operation that deletes, redacts or prunes history, and none is planned in
place. Retracting a fact ends its interval; it does not remove the value. Decide
before writing whether a value may be kept forever.

The decision and the options considered are in
[decision brief 06](decision-briefs/06-history-excision.md).

## Why history cannot change

Several guarantees depend on retained history staying exactly as committed:

- A snapshot token names a revision, and every read at that token returns the
  same answer, after reopen and from backups.
- Every full open replays and validates the retained log and checks the datom
  index against it.
- A keyed retry compares the stored canonical request bytes and returns the
  stored original report.

## Where a value is stored

A value written through Factlite can appear in all of these places:

- the authoritative fact events;
- the derived datom intervals and their indexes;
- the encoded report of the transaction that committed it, as an effective
  delta;
- for a request with an idempotency key, the stored canonical request. That
  request also holds values that never became facts: expectation values,
  lookup terms (`EntityTerm::Lookup`, `ValueTerm::LookupRef`) and operations
  that had no effect;
- SQLite free pages and the WAL file, after the rows that held it change;
- every backup made after it was committed, and every copy of those files.

Values are not the only retained input. Attribute names, temporary entity
labels, metadata keys and values, and idempotency keys are stored in reports or
retry records and are permanent too.

Errors can also carry values to the caller: `PreconditionFailed` reports the
expected and actual sets, `UniqueViolation` the contested value, `ConflictingBatch`
can report the conflicting fact, and a failed lookup reports its key. An
application that logs errors, or CLI stderr, copies those values into its own
logs.

A rejected request, or a commit error whose outcome is confirmed rolled back,
adds no committed records. A commit error with an unknown outcome may already
have committed; retry the original keyed request to find out.

## Keep sensitive data out of fact values

Do not write secrets, credentials or personal data that may have to be deleted
as fact values, metadata, labels or idempotency keys. Store the data in a
record the application can delete, and store a reference to it in Factlite,
such as an opaque random identifier as a `string` or `bytes` value. Deleting
the external record leaves the reference in history, pointing at nothing.

Do not derive the reference from the data. A hash of an email address, for
example, can be recovered by hashing candidate addresses.

## Application-level encryption

When the data has to live in the store, encrypt it in the application and store
the ciphertext as a `bytes` value, with the key held outside Factlite. A key per
subject (per person, per tenant) lets the application make that subject's data
unreadable by destroying its key, including the copies in history, free pages,
the WAL and backups. This is sometimes called crypto-shredding. Factlite needs
no key: it sees opaque bytes, and replay, validation and backup work unchanged.

Factlite does not provide encryption or key management. The pattern has these
limits.

### Equality and uniqueness

Factlite compares the stored bytes exactly. It never sees plaintext.

- With randomized encryption, two encryptions of the same plaintext are
  different values. Asserting the same plaintext again adds a new value to a
  many-valued attribute, or replaces a one-valued attribute with a new value
  in a new commit. Retracting a plaintext requires the exact ciphertext that
  was stored. Retracting a fresh encryption leaves the stored fact in place,
  while the request still commits a new revision, and a keyed request keeps
  the submitted ciphertext in its retry record. Read the stored value first.
- Expectations compare the exact pre-transaction set of ciphertexts. Expect
  the ciphertext you read, not a fresh
  encryption of the value you believe is there.
- A `unique_one` attribute holding ciphertext enforces uniqueness of
  ciphertext, not plaintext, in the final state of each transaction. Two
  subjects can hold the same plaintext.
- Deterministic encryption gives equal ciphertext for equal plaintext only
  under one key and context. With per-subject keys it still cannot enforce
  plaintext uniqueness across subjects, and it reveals which values are equal
  within a subject.
- Queries, predicates, ordering and aggregates operate on ciphertext. Encrypted
  values cannot be searched, compared or sorted by content. Query on
  unencrypted attributes and decrypt the results in the application.
- Do not use an encrypted attribute as the target of a lookup ref unless the
  application can reproduce the exact stored ciphertext.

### Retries

A keyed retry must send the original request, including the original
ciphertext. If the original request committed and encrypting again after a
lost response produces different ciphertext, the retry has different canonical
bytes and fails `IdempotencyConflict` instead of returning the original report.
If the original never committed, the re-encrypted request commits as new. Keep
the complete encrypted request, not only the plaintext, until the outcome is
known.

### When destroying a key is enough

Destroying a key makes the data unreadable only if all of these hold:

- every copy of the key is destroyed, including key backups and any cache;
- the plaintext was never committed, in an earlier transaction, as a fact
  value, metadata, a label, an idempotency key, a lookup term or an expectation;
- the plaintext was never copied elsewhere: exports, query results saved by the
  application, logs, error output;
- the ciphertext and its surrounding facts reveal nothing the application must
  delete. Entity identities, attribute names, references, revisions and the
  shape of each transaction stay readable.

The encrypted bytes remain in history, so reads at old snapshots still return
them, and validation, retries and backups keep working.

## Physical deletion

Factlite has no operation that removes committed data. Data that was committed
by mistake can only be removed outside Factlite, and doing so loses history:

- **Sanitized export:** an application migration, not a Factlite command. Read
  the current state, leave out the data to be removed (including any sensitive
  attribute names, labels or metadata), write the rest into a fresh store, and
  destroy the old one and its copies. Entities get new IDs, so references must
  be remapped. History and old snapshot tokens are lost.
- **Whole-store destruction:** delete the file and every copy.
- **Lineage rewrite:** a tool that replays history into a new lineage without
  the removed facts. It is not designed or built; it would be designed if a real
  deletion requirement appears. Editing rows in place is ruled out, because it
  would change what existing tokens and retained retry records mean.

Physical removal of bytes is a separate problem:

- Deleting a file is not erasure. Filesystem snapshots, remnants on SSDs and
  other storage, and earlier backups keep the data. `Store::backup` copies all
  history through its captured revision without sanitizing it.
- SQLite does not erase rows when they are deleted or updated. Factlite never
  deletes rows, does not set the `secure_delete` pragma (its default depends on
  the SQLite build) and never runs `VACUUM`, so freed pages can keep old
  content. Any future tool that removes rows inside a file would need
  `secure_delete=ON` before the removal, or `VACUUM` afterwards to also clear
  pages freed earlier. Neither removes values still retained in history,
  reports or retry records.
- Writable stores use WAL mode; standalone backups use DELETE journal mode. An
  ordinary checkpoint can leave old frames in the `-wal` file. A truncating
  checkpoint removes them only when it completes, and active readers can
  prevent that; check its busy result.

Plan for deletion before writing data: keep it out of the store, or encrypt it
under a key the application can destroy.
