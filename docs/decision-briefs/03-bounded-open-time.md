# Decision brief: bounded open time (roadmap item 3)

Status: decided 2026-11-06 (see Decision). Not yet implemented.

## The question

Every open, writable or read-only, fully validates the retained log before
returning. Its cost grows with history and live state. Should open keep doing
that, and if not, how is integrity established instead?

## What open does today

`Store::open` and `Db::open_read_only` run `validate_current`
(`crates/factlite/src/store.rs`) inside one read transaction:

1. Table and index sets, the head row, and foreign keys.
2. `log::replay`: every transaction is decoded, its event rows are checked
   against its report, and its state transition is replayed and validated
   incrementally (item 9.4). Keyed records are re-evaluated from their stored
   canonical request.
3. `index::verify`: the stored datom intervals must equal intervals derived from
   all fact events.
4. `index::verify_against`: the indexed state at head must equal the replayed
   state.

Reads and commits after open no longer depend on history (items 9.2, 9.4,
9.5). Open is the main operation whose cost grows with stored history. Two
others do too: unfiltered `Db::history` reads every report in its range, and
`Store::backup` fully validates both the source and the copy.

## Measurements (2026-11-05, Apple M4 Pro, release build)

| Store | Revisions | Fact events | Open (CLI read, median of 3) |
| --- | ---: | ---: | ---: |
| clajent kit copy | 3,205 | 24,452 | 0.14 s |
| Synthetic, 40,000 tasks | 874 | 121,744 | 0.67 s |

A sampled profile of the synthetic open split validation as:

| Phase | Share |
| --- | ---: |
| replay (dominated by applying the 120,000-fact seed commit) | 53% |
| datom interval verification | 20% |
| comparing indexed with replayed state | 22% |
| other | 5% |

So every phase is proportional to stored rows or live state, not to revision
count alone. These are three samples per store on a warm cache, with no
separation of open from the read command. The synthetic history is dominated by
one large seed commit, so it says little about histories with many retractions
or many small commits. Reproducible workloads are needed before choosing D or B. A full byte-level hash scan of the transaction and fact-event
tables (a Python prototype, SHA-256) took 155 ms on the same store. A Rust
implementation would be faster, but this has not been measured.

The cost shows up most in per-process use. The CLI opens the store for every
command, so a script of many CLI commits pays the full open each time: building
the synthetic store with one CLI process per commit managed about 870 commits
in 10 minutes. Long-lived processes, such as the pod and the planned JSON-RPC
server (roadmap item 10), pay it once.

## Options

### A. No change

Open keeps full validation.

- **Integrity:** strongest. Every open checks every row semantically.
- **Cost:** linear in history and live state: 0.14 s on the real kit store,
  0.67 s at 120,000 facts, and growing.
- **Work:** none.

### B. Hash-chained checkpoints

Each transaction row stores a chained hash of its row and event bytes and the
previous hash (a format change). A checkpoint records "fully validated through
revision C, chain hash h". Open would:

1. Recompute the chain over all rows: a byte scan, no model work.
2. Check that it reaches h at C.
3. Semantically replay only revisions after C, starting from the indexed state
   at C.
4. Re-derive and compare datom intervals, which change for old rows whenever a
   later commit retracts a fact.

- **Integrity:** byte changes anywhere in the log are still detected. Semantic
  validity of rows up to C rests on the validation that wrote the checkpoint.
- **Hash coverage:** must include schema and entity events, row order,
  encodings and store identity, not just reports.
- **Trust model:** the checkpoint hash lives in the same file as the data, so it
  detects accidental corruption and partial copies, not coordinated edits.
- **Cost:** still linear in stored bytes. Interval verification, the head
  comparison and other checks (47% of today's time) remain, plus the scan.
  With the 155 ms Python scan that is roughly 70% of today's 0.67 s. A Rust
  scan would be faster, but no larger gain is shown unless some checks are
  dropped and that is justified.
- **Work:** large: format 5, chain maintenance in every commit, checkpoint
  writes, upgrade, tamper tests. It is new derived state that must itself be
  verified.

### C. Explicit verify, cheap open

Open checks only structure: tables, indexes, the head row and the format.
Full validation moves to `Store::verify`, `Db::verify` and a CLI `verify`
command, run when the caller chooses (after a restore, in a periodic job,
before a backup).

- **Integrity:** corruption goes undetected until verify runs, and reads in the
  meantime could return wrong answers from a corrupted index.
- **Cost:** open becomes near constant (milliseconds).
- **Work:** small.

### D. Validated-revision marker

Like C, but with protection for new commits. A marker records the last revision
fully validated (by verify, or by a full open). Open then replays and checks
only the commits after it, so commits made by other writers since the last
validation are still checked semantically.

- **Integrity:** between C and A, but only with more design than shown here.
  The marker does not authenticate the state at the marked revision. A fact
  asserted before the marker and retracted after it updates an older interval
  row, so D must say how those changes and historical intervals are checked.
- **Cost:** commits since the last validation, plus loading the indexed state
  at the marker (`index::model_at`), which is proportional to live state.
- **Work:** medium to large: a marker row, a partial replay from the indexed
  state, interval checks across the marker, and verify.

### E. Make the existing validation faster

Keep A's guarantees and reduce constant factors. For example, the 22% head
comparison could reuse work already done by replay, and interval verification
could compare in SQL instead of building both sets in Rust.

- **Integrity:** unchanged.
- **Cost:** still linear, by a smaller factor; not measured.
- **Work:** small to medium, with parity tests like item 7.3's.

## Recommendation

**A now, plus the explicit `verify` operation from C, without changing open's
default.** Keep full validation at open for now:

- On the real consumer's store, open costs 0.14 s.
- The pod is long-lived and pays it once.
- Repeated CLI commands do pay it every time, as the store build above showed.
  The JSON-RPC server would remove that cost, but it is deferred and not
  authorized, so this decision does not rely on it.

Then, in order:

1. **Add verify.** Add `Store::verify`, `Db::verify` and a CLI `verify`
   command. They are useful on their own, after a restore and for stored
   fixtures, and are the foundation for C or D. Their contract: verify the
   latest committed snapshot in one read transaction, and return that snapshot
   or a `Storage` error naming the failed check. An open followed by verify
   validates twice; that is acceptable for an explicit command.
2. **Try E first** if open cost becomes a problem, because it keeps every
   guarantee.
3. **Consider D** as an opt-in open option only after reproducible workloads
   show a consumer needs it, and only with the interval design above.
4. **Avoid B** unless D proves insufficient: it adds the most derived state for
   a gain the measurements do not show.

## Second opinion

A Codex second opinion on this brief (2026-11-05, through kit) is recorded
below.

Codex reviewed the first draft. It agreed with keeping full validation by
default, and found the description of open substantially accurate. This
revision adopts its other points:

- **D was undercosted and underspecified.** Loading the state at the marker is
  proportional to live state, and later retractions rewrite older interval
  rows.
- **B's speed estimate was unsupported.** Retained checks plus the scan come to
  about 70% of today, not a third to a half. B also needs an explicit hash
  coverage and trust model.
- **The deferred server should not support the decision.**
- **Open is not the only history-dependent operation.** Unfiltered history and
  backup are too.
- **The measurements justify caution, not scaling predictions.** Reproducible
  workloads should precede D.
- **Optimizing the existing passes (E) was a missing option.**
- **verify needs a defined contract.**

## Decision (user, 2026-11-06)

- **Default:** keep full validation on every open (option A).
- **Opt-in:** a `Structural` validation option in `StoreOptions`, allowed for
  read-only opens only. It checks format markers, the table and index sets,
  and that the head matches the latest transaction.
- **Writable opens:** they always validate fully, because a commit on an
  unvalidated index could write a wrong result into the authoritative log.
- **Verify:** add an explicit `Store::verify` and `Db::verify`, a CLI `verify`
  command and `--validation structural` flag, and pod `:validation` and
  `verify`.
- **Plan item:** 12.1.
