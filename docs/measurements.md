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

## Decision

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
