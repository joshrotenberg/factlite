# Workload baseline and optimization decision

Run the release example on a directory backed by the filesystem you intend to
measure. It creates and cleans private database directories and emits one JSON
record per workload:

```sh
mkdir -p target/benchmark-files
cargo run --release --locked -p factlite --example workloads -- \
  --directory target/benchmark-files
cargo run --release --locked -p factlite --example workloads -- \
  --tasks 100 --revisions 500 --samples 31 --directory target/benchmark-files
```

Without a directory option it uses the OS temporary directory. Counts must be
positive; custom configurations require tasks and revisions. Unknown, missing and
duplicate options fail. No database is retained after the run.

## Workload and method

One person owns N tasks; each task has title, status and owner, with half initially
doing. Creation is one keyed schema/allocation/fact commit. Subsequent keyed
revisions alternate the first task's status. The joined query selects task/title/
owner name, filters doing, orders title plus identity, and limits to 20 distinct
rows. Historical queries pin the creation revision; queries at captured head
pin the revision after all replacements.

Every timed query is checked against independently calculated typed expected
rows. Head, entity/fact counts, no-op deltas/revisions and unchanged pinned answers
are also asserted. Request construction, setup, warmup and assertions are outside
operation timings. Warm each query once, then take 15 samples for each operation;
report median and nearest-rank p95. The initial seed commit is a single sample.
No-op commit samples advance history from R+1 through R+15, so their replay work
grows during the sample set. Read-only open includes full retained-log validation;
head capture does not. CLI startup/rendering is not included.

## Recorded run: 2026-10-04

Linux 6.18.44 x86_64, virtual AMD EPYC 7763, five visible CPUs with a four-core
cgroup quota; Rust 1.99.0, release optimization, bundled SQLite 3.53.2, WAL/FULL,
foreign keys and two-second busy timeout. `/workspace` is an overlay filesystem;
`/tmp` is tmpfs. These are warm local-container measurements, not a physical-disk
fsync/power-loss characterization or a production latency promise.

Raw outputs retain full median/p95, sample counts, inputs and SQLite version:
[workspace](benchmarks/2026-10-04-workspace.jsonl),
[tmpfs](benchmarks/2026-10-04-tmpfs.jsonl). All timings below are workspace medians
in milliseconds, rounded to three decimals. Each graph has N+1 entities and
3N+1 facts.

| Tasks / revision | Head | Read-only open | Query at revision 1 | Query at captured head | Facts at head | New no-op commit |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 10 / 10 | 0.016 | 1.137 | 0.514 | 1.179 | 0.908 | 2.340 |
| 100 / 50 | 0.021 | 19.965 | 2.576 | 20.142 | 19.528 | 23.958 |
| 1,000 / 50 | 0.025 | 178.738 | 23.646 | 181.939 | 177.552 | 203.272 |
| 100 / 250 | 0.021 | 89.225 | 2.610 | 89.065 | 88.449 | 92.505 |

For the 1,000/50 workload, head-query p95 was 183.326 ms and no-op commit p95
271.591 ms. Fifteen samples give only a coarse tail estimate. Tmpfs head-query
medians were 1.127/19.528/181.961/89.027 ms for these four cases. Overlay write
latency differs, but both runs show the same strong dependence on replayed
revision count and live state size. Historical revision-1 queries remain bounded
by that old prefix after head advances.

## Baseline decision (6.4)

Retain the correctness-first replay path and existing indexes for this increment.
No latency budget, expected production graph/history size, or concurrency target
has been established. The baseline exposes a concrete cost rather than declaring
it acceptable for every application: full-log validation/reconstruction dominates
reads and commits as retained history grows. A CLI read also opens/validates before
performing its operation, so its complete elapsed time can include replay twice.

Extra query indexes are not justified by these observations. The TEMP fact primary
key already starts with resolved attribute ID, while joined query times are close
to replayed fact-read and file-open times. Durable log tables already index revision/
ordinal and unique identity/attribute/key lookups. Preserve them without speculative
DDL or format changes.

The next performance proposal should target repeated replay and validation, using
an explicitly bounded immutable-state cache or verified checkpoints after an
actual workload and latency budget are selected. Any proposal must retain all
history, historical schema, canonical retry evidence, owned views and short SQL
snapshots; publish derived state only after successful commits, compare against
replay, and repeat forced writer/failure/reopen/backup checks. No cache, checkpoint
or durable projection was added, so no optimized storage path is presented as
validated. There is no invented performance target to close this item against.

## Replay profiling and evaluated-state reuse (7.3): 2026-10-05

The next bounded improvement removes redundant reconstruction rather than
introducing derived storage. Previously each keyed record evaluated its retained
canonical request, checked the complete generated report, discarded the resulting
state, then reconstructed and validated the same state from the report. Replay
now retains the evaluated state after exact report equality. All format, identity,
event-row and canonical-request checks still run; unkeyed records use report
application. SQL transaction boundaries and the storage format are unchanged.

Run the manual stage harness in release mode, with an absolute output directory
(Cargo tests run from the crate directory):

```sh
mkdir -p target/benchmark-files
FACTLITE_PROFILE_DIRECTORY="$PWD/target/benchmark-files" \
  cargo test --release --locked -p factlite --lib \
  log::profile::retained_replay_stage_profile -- --ignored --exact --nocapture
```

The harness always measures the original double reconstruction, even after the
optimization. Each node has integer/title/reference facts, references form a
cycle, and later keyed revisions are metadata-only commits. It warms once and
takes fifteen samples inside one fixed SQL read snapshot. Setup is excluded.
It preloads transaction rows, times decode, event checks, evaluation (including
discarded-state cleanup) and report application separately, and checks revision,
schema and final counts. Wall time includes assertions and remaining cleanup;
phase medians need not sum to the wall median. This is stage instrumentation of
a related fixture, not an exact breakdown of the task-query workload.

[Recorded stages](benchmarks/2026-10-05-replay-stages.jsonl), in milliseconds:

| Nodes / revision | Decode | Events | Request evaluation | Report application | Wall |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1,000 / 50 | 5.298 | 5.172 | 64.032 | 83.710 | 170.244 |
| 100 / 250 | 1.544 | 8.129 | 29.450 | 40.513 | 83.294 |

The redundant report application is about half the profiled wall time. Fresh
full workload runs compare pre-optimization `bb1d0a6` with item 7.3 on the same
workspace overlay, using the runner and fifteen samples described above. The
benchmark runs were separate from tests and other builds. Raw outputs:
[before](benchmarks/2026-10-05-workspace-before.jsonl),
[after](benchmarks/2026-10-05-workspace-after.jsonl).

| Tasks / revision | Read-only open before → after | Head query before → after | Facts before → after | No-op commit before → after |
| --- | ---: | ---: | ---: | ---: |
| 10 / 10 | 1.115 → 0.900 | 1.099 → 0.898 | 0.884 → 0.639 | 2.095 → 1.601 |
| 100 / 50 | 19.605 → 9.981 | 19.701 → 10.238 | 19.341 → 9.648 | 23.232 → 11.601 |
| 1,000 / 50 | 177.585 → 82.630 | 180.394 → 85.531 | 176.707 → 81.259 | 200.195 → 92.159 |
| 100 / 250 | 90.072 → 43.169 | 89.986 → 43.527 | 89.998 → 42.782 | 94.076 → 45.914 |

All table times are medians in milliseconds. Head-query medians fell 18–53% in
these four runs; at 1,000/50 its p95 changed from 182.865 to 86.481ms. These are
observed warm-container measurements with the same environment limits as the
baseline, not an SLA. Unkeyed records do not gain this shortcut, and even keyed
replay still traverses all retained revisions. Immutable caches/checkpoints and
production workload budgets remain future decisions.

The original replay path remains in test builds for typed randomized parity and
future-allocation checks. Independent domain traces, corruption, rollback,
forced writer isolation, abrupt process recovery, backup and CLI checks are the
semantic gate for this change. See `reviews/0030-verified-replay-state-reuse.md`.

## Temporal datom index (9.1): 2026-11-05

Item 9.1 loads the state at R from revision-bounded schema/entity events and
datom intervals instead of replaying the log for each read and commit. Open still
replays and validates the full log, and now also verifies the index.

Both runs used the unchanged workload runner on the same host, one after the
other, with the release build of `main` at `7bc5d2f` and of the 9.1 branch.
Host: Apple M4 Pro, 14 CPUs, macOS 27.2, APFS internal SSD, Rust 1.99.1,
bundled SQLite 3.53.2, WAL/FULL. These are warm local observations, not an SLA,
and are not comparable to the earlier Linux container tables above. Raw outputs:
[before](benchmarks/2026-11-05-macos-before.jsonl),
[after](benchmarks/2026-11-05-macos-after.jsonl). Medians in milliseconds:

| Tasks / revision | Read-only open | Query at revision 1 | Query at captured head | Facts at head | New no-op commit |
| --- | ---: | ---: | ---: | ---: | ---: |
| 10 / 10 | 0.357 → 0.456 | 0.158 → 0.104 | 0.316 → 0.103 | 0.254 → 0.038 | 0.503 → 0.136 |
| 100 / 50 | 4.675 → 5.226 | 1.041 → 0.512 | 4.726 → 0.504 | 4.451 → 0.255 | 5.074 → 0.384 |
| 1,000 / 50 | 39.266 → 44.892 | 10.066 → 4.942 | 41.088 → 4.922 | 38.540 → 2.499 | 45.124 → 2.901 |
| 100 / 250 | 19.748 → 20.656 | 1.140 → 0.616 | 19.703 → 0.547 | 19.327 → 0.271 | 19.555 → 0.361 |

At 1,000 tasks/50 revisions, head-query p95 changed from 42.085 to 5.140ms and
no-op commit p95 from 48.279 to 4.305ms. Query, fact and commit costs now track
live state size rather than retained revision count: the 100-task workloads at
50 and 250 revisions are close. Read-only open is 5–28% slower because it adds
index derivation/comparison and an indexed-state cross-check to full replay. CLI
commands open the file per process, so their elapsed time still includes full
replay; only long-lived `Store`/`Db` handles gain the per-operation improvement.
Queries still copy the visible state into a TEMP relation (item 9.2).

## Incremental replay validation (9.4): 2026-11-05

Same host and runner as the 9.1 table, `main` at `f8c2cf0` against the 9.4
branch. Raw outputs: [before](benchmarks/2026-11-05-macos-before-9.4.jsonl),
[after](benchmarks/2026-11-05-macos-after-9.4.jsonl). Medians in milliseconds:

| Tasks / revision | Read-only open | Query at revision 1 | Query at captured head | Facts at head | New no-op commit |
| --- | ---: | ---: | ---: | ---: | ---: |
| 10 / 10 | 0.592 → 0.433 | 0.131 → 0.111 | 0.127 → 0.107 | 0.044 → 0.038 | 0.250 → 0.157 |
| 100 / 50 | 5.437 → 2.436 | 0.597 → 0.551 | 0.540 → 0.520 | 0.263 → 0.258 | 0.485 → 0.440 |
| 1,000 / 50 | 45.770 → 16.439 | 5.133 → 5.077 | 5.152 → 5.079 | 2.571 → 2.539 | 3.096 → 2.915 |
| 100 / 250 | 20.890 → 5.272 | 0.630 → 0.545 | 0.562 → 0.521 | 0.273 → 0.260 | 0.377 → 0.360 |

Open scaling at 1,000 tasks, five samples each, read-only open medians:

| Revisions | Before | After |
| ---: | ---: | ---: |
| 500 | 331.7 | 23.5 |
| 2,000 | 1,291.5 | 46.3 |
| 8,000 | not run | 138.2 |

Open is now linear in retained revisions, at roughly 15 µs per revision for this
workload, plus loading the state. On a backup copy of the clajent session's
store (1,401 entities, 2,803 revisions, 18,650 live facts), a CLI read that
opens the file went from about 7.5 s to 0.14 s.

Commits and reads are still linear in live state, because they load the full
state at head. At 10 revisions with 3,001 / 30,001 / 120,001 facts, no-op commit
medians were 3.1 / 30.9 / 162.8 ms, and head-query medians 5.2 / 59.7 / 359.6 ms.

## Direct indexed reads (9.2): 2026-11-05

`main` at `fb2005c` against the 9.2 branch, ten revisions, seven samples.
Raw outputs: [before](benchmarks/2026-11-05-macos-before-9.2.jsonl),
[after](benchmarks/2026-11-05-macos-after-9.2.jsonl). Medians in milliseconds:

| Facts | Query at captured head | Query at revision 1 | All facts at head | No-op commit |
| ---: | ---: | ---: | ---: | ---: |
| 3,001 | 5.173 → 0.120 | 5.366 → 0.184 | 2.76 → 1.58 | 3.09 → 2.82 |
| 30,001 | 61.863 → 0.123 | 63.287 → 0.235 | 30.73 → 15.93 | 34.39 → 33.84 |
| 120,001 | 394.625 → 0.131 | 395.457 → 0.374 | 147.46 → 65.88 | 172.68 → 168.98 |

The selective joined query no longer depends on live state size. An
unfiltered `facts` read still returns every visible fact. Commits are unchanged
by this item.

## Commits load only touched state (9.5): 2026-11-05

`main` at `7fb07ea` against the 9.5 branch. Raw outputs:
[before](benchmarks/2026-11-05-macos-before-9.5.jsonl),
[after](benchmarks/2026-11-05-macos-after-9.5.jsonl). No-op commit medians:
3.0 → 0.14 ms at 3,001 facts, 34.7 → 0.10 ms at 30,001, and 173.5 → 0.09 ms at
120,001.

Through the pod, on a backup copy of the clajent store, 200 iterations of a
keyed 13-fact create (with a unique key) and a conditional finish had p50
latencies of 0.52 ms and 0.23 ms, including the babashka round trip. That open
also upgraded the file from format 2 to 3 and took 184 ms.

Open is now the only operation linear in live state: 870 ms at 120,001 facts.

## Indexed filtered history (11.3): 2026-11-05

Through the pod on the clajent store copy (24,452 effective events), history
p50 over 20 calls, `main` at `45d3e28` against the 11.3 branch: entity and
attribute 16.28 → 0.12 ms (3 events), attribute only 19.24 → 3.43 ms (600
events), unfiltered 129.9 → 130.3 ms (unchanged path).

## Structural read-only open (12.1): 2026-11-06

Median of seven CLI `schema` runs, including process start. On the 120,000-fact
synthetic store (875 revisions), full validation took 774 ms and structural
3.2 ms. On a copy of the clajent store (3,205 revisions), full validation took
156 ms and structural 4.1 ms. `verify` costs the same as a full open.

## Work queue pattern (13.1): 2026-11-23

The `work_queue` example's measured mode, on the `docs/queue-and-agent-guides`
branch. Apple M4 Pro, macOS 27.2.1, Rust 1.99.0, release build, OS temporary
directory. Raw output:
[benchmarks/2026-11-23-macos-work-queue.jsonl](benchmarks/2026-11-23-macos-work-queue.jsonl).

```sh
cargo run --release --locked -p factlite --example work_queue -- \
  --workers 4 --jobs 2000 --spread 8
```

The run enqueues every job with a distinct due time in the past, starts the
worker processes, and has them begin claiming at the same instant. Each worker
claims, reads the job and finishes it, with no work in between. `drain_ms`
runs from that instant to the last worker's exit. One run per configuration.

| Workers | Jobs | Spread | Drain ms | Jobs per second | Lost claims | Candidate read ms, start and end | File bytes | Writable open ms |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 500 | 1 | 422 | 1,185 | 0 | 0.215, 0.463 | 4,448,256 | 43.1 |
| 1 | 2,000 | 1 | 2,698 | 741 | 0 | 0.215, 1.537 | 17,498,112 | 184.0 |
| 4 | 2,000 | 1 | 3,055 | 655 | 2,300 | 0.219, 1.540 | 17,522,688 | 183.0 |
| 4 | 2,000 | 8 | 2,029 | 986 | 256 | 0.213, 1.567 | 17,448,960 | 185.3 |
| 8 | 2,000 | 16 | 2,085 | 959 | 228 | 0.216, 1.587 | 17,645,568 | 185.1 |
| 1 | 8,000 | 1 | 30,967 | 258 | 0 | 0.216, 6.551 | 69,840,896 | 776.0 |
| 4 | 8,000 | 8 | 14,516 | 551 | 2,165 | 0.222, 9.836 | 69,840,896 | 951.1 |

Each job is three revisions (`3 * jobs + 2` in every run) and about 8.7 KB of
file. A writable open cost about 31 to 40 microseconds per revision.

The candidate read is the median of 21 calls to the read that starts every
claim, taken with every job pending (start) and with every job finished
(end). At the start it was the same at 500, 2,000 and 8,000 pending jobs. At
the end it was about 0.8 ms per 1,000 finished jobs in five runs and 1.2 ms
in the four-worker run over 8,000. The runs were made on the commit before
the review changes to the pattern; those changes do not alter the queries.

An earlier form of this run gave every job the same due time. The read then
also grew with the number of pending jobs, 1.9 ms at 2,000, because ordering
by due time and then by job had to read the whole tie. Distinct due times, as
jobs enqueued over time have, removed that.

A second measurement, with a test that is not in the repository, timed
variants of the pending-jobs read on stores with 50 pending jobs and 0, 2,000
or 8,000 finished ones. Mean of 30 calls, in milliseconds:

| Read | 0 finished | 2,000 | 8,000 |
| --- | ---: | ---: | ---: |
| As in the pattern: queue, state, due time, attempt; due-time predicate; ordered; limit 1 | 0.117 | 1.562 | 6.536 |
| Without the predicate | 0.104 | 1.489 | 6.486 |
| Without predicate, order or limit | 0.116 | 1.521 | 6.372 |
| Queue and state patterns only | 0.056 | 0.990 | 4.416 |
| State pattern only | 0.038 | 0.147 | 0.459 |
| Pull of one job by entity | 0.027 | 0.028 | 0.028 |

Every read that joins two or more patterns grows at about the same rate,
whatever its predicate, order or limit. The single-pattern read grows about
fourteen times more slowly, and a pull by entity not at all. The join is
therefore driven from an attribute that every job holds, and the cost is in
visiting finished jobs, not in finding pending ones. The SQLite query plan
was not captured. A reviewer who compiled the same SQL on plain SQLite 3.45.3
found it driven from the due-time range of the attribute and value index.

An experiment that is also not in the repository added
`CREATE INDEX ... ON datoms(attribute, value, retracted, entity, asserted)`
to a file with 4,000 finished and 100 pending jobs and ran `ANALYZE`. The
pending-jobs query without its `job/queue` pattern went from 1.99 ms to
0.21 ms. With the `job/queue` pattern, which matches every job, it went from
3.0 ms to 3.9 ms. `ANALYZE` alone changed nothing. Reordering the query's
patterns to put `job/state` first, with no new index, gave 2.0 ms against
3.0 ms.
