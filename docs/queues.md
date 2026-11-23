# Work queues

A durable work queue can be written on Factlite's public API with no library
support. This guide gives one pattern, what it guarantees, what it costs, and
where it stops fitting. Plan item 13.1.

The pattern is one file:
[`examples/work_queue/queue.rs`](../crates/factlite/examples/work_queue/queue.rs).
The [example](../crates/factlite/examples/work_queue/main.rs) and the
[tests](../crates/factlite/tests/work_queue.rs) include that same file, so what
is described here is what runs. It is not part of the library.

```sh
cargo run -p factlite --example work_queue
cargo run --release -p factlite --example work_queue -- --workers 4 --jobs 2000 --spread 8
```

The first command walks through one queue and checks each step. The second
starts worker processes on one file and prints what it measured.

## When this fits

It fits work in the thousands of jobs per store file, on one host, where the
record of who did what matters and retries must not duplicate work: agent
tasks, runs and handoffs.

It does not fit a high-throughput broker, a queue that keeps a long backlog of
finished jobs in one file, workers on several machines, or payloads that must
later be deleted. The [costs](#costs) below say why.

## The model

A job is an entity. Every attribute holds one value.

| Attribute | Type | Meaning |
| --- | --- | --- |
| `job/id` | string, unique | The producer's name for the job. Unique across the store, not per queue. |
| `job/queue` | keyword | Which queue the job is in. |
| `job/state` | keyword | `pending`, `claimed`, `done` or `failed`. |
| `job/payload` | string | What to do. Permanent; see [permanence](#permanence). |
| `job/run-after` | inst | When the job becomes claimable. Held only while pending. |
| `job/attempt` | i64 | Claims made so far. The fence. |
| `job/owner` | string | The last worker to claim the job. |
| `job/lease-until` | inst | When another worker may take the job. Held only while claimed. |
| `job/result` | string | Set by finish or fail. |

A job moves from `pending` to `claimed` and then to `done` or `failed`. A
claimed job can also go back to `pending` (release), or stay `claimed` under a
new owner and the next attempt (takeover).

A job holds a due time only while pending and a lease only while claimed.
Claiming retracts the due time; finishing, failing and releasing retract the
lease. The values stay in history. This is what keeps finished jobs out of
the reads that find claimable work; see [costs](#costs).

Factlite records no clock. Every function takes the current time from its
caller.

## Operations

Each operation is one commit. "Expects" lists the exact values the job must
hold for the commit to be accepted.

| Operation | Expects | Writes | Idempotency key |
| --- | --- | --- | --- |
| `enqueue` | nothing | a new pending job at attempt 0 | from `job/id` |
| `try_claim`, pending job | state `pending`, attempt as read | state `claimed`, owner, lease, attempt + 1; retracts the due time | none |
| `try_claim`, expired job | state `claimed`, lease and attempt as read | owner, lease, attempt + 1 | none |
| `heartbeat` | state `claimed`, owner, attempt, lease | a later lease, never an earlier one | none |
| `finish` | state `claimed`, owner, attempt, lease | state `done`, result; retracts the lease | from job and attempt |
| `fail` | state `claimed`, owner, attempt, lease | state `failed`, result; retracts the lease | none |
| `release` | state `claimed`, owner, attempt, lease | state `pending`, a new due time; retracts the lease | none |

`candidates` reads the jobs claimable at one snapshot: pending jobs that are
due, oldest first, then claimed jobs whose lease has passed. Factlite queries
have no disjunction, so it is two queries against the same view, each with its
own limit. `try_claim` refuses, without committing, a candidate that is not
claimable at the time it is given. `claim` reads candidates, tries one, and
reads again if it lost.

A claim belongs to one thread of control. Each heartbeat returns the claim to
use from then on. A copy from before a heartbeat no longer matches the stored
lease and is refused like a lost claim.

Only enqueue and finish are keyed. A repeated `fail` or `release` whose first
call committed returns `false`, because the job is no longer claimed; read the
job to tell that apart from a lost claim.

A failed expectation is `PreconditionFailed`. The pattern turns that into "no
claim" or `false`. Nothing is committed and the head does not move.

## What is guaranteed

Each line names the test in
[`tests/work_queue.rs`](../crates/factlite/tests/work_queue.rs) that shows it.

- **A job is enqueued once.** Repeating an enqueue returns the same entity and
  commits nothing. The same `job/id` with different contents is refused with
  `IdempotencyConflict`, and without a key the unique `job/id` refuses it with
  `UniqueViolation`.
  `enqueue_is_exact_on_retry_and_refuses_a_changed_or_duplicate_job`.
- **One claim per attempt.** When several workers try the same job, one
  commits. The others commit nothing.
  `processes_that_read_the_same_job_commit_exactly_one_claim` has four
  processes read the same candidate, waits until all four hold it, and then
  lets them claim: one wins, three lose, and one revision is added.
- **Every job is finished exactly once by several processes.**
  `worker_processes_finish_each_job_exactly_once` runs four worker processes
  on one file over 100 jobs. Every job ends with one claim and one finish,
  and the file holds no revision beyond those. This test does not force
  contention; the one above does.
- **Only the current holder can write.** Heartbeat, finish, fail and release
  are accepted only while the job is claimed, from the owner, at the current
  attempt and lease. The attempt is needed as well as the owner and lease: a
  worker that releases a job and claims it again at the same instant holds
  two claims that differ only in the attempt.
  `an_expired_lease_is_taken_and_the_old_claim_can_no_longer_commit`,
  `a_worker_that_claims_its_own_expired_job_again_loses_the_earlier_claim`,
  `a_claim_from_before_a_release_is_refused_even_with_the_same_owner_and_lease`.
- **A takeover needs an expired lease and loses races.** It loses to a
  heartbeat because it expects the lease it read, and to a late finish, fail
  or release because it expects the job to still be claimed.
  `a_heartbeat_between_read_and_reclaim_wins`,
  `a_takeover_loses_to_a_late_finish_fail_or_release`.
- **A stale read cannot claim.** A candidate read before the job was claimed
  and released is refused, because the attempt moved. A candidate is also
  refused before the time it was read for.
  `a_stale_pending_candidate_does_not_claim_a_released_job`,
  `a_candidate_is_not_claimed_before_the_time_it_was_read_for`.
- **A heartbeat never shortens a lease.**
  `a_heartbeat_never_shortens_the_lease`.
- **A due time is held only while pending and a lease only while claimed.**
  `a_job_holds_a_due_time_only_while_pending_and_a_lease_only_while_claimed`.
- **The original enqueue stays exact after the job is rescheduled,** and a job
  id is refused in a second queue.
  `the_original_enqueue_stays_exact_after_the_job_is_rescheduled`.
- **Due jobs come before expired ones, in one order.**
  `due_time_and_order_decide_what_is_claimable`,
  `due_jobs_come_before_expired_ones_and_ties_have_one_order`.
- **A finished or failed job never changes** and is never claimable. The end
  of `an_expired_lease_is_taken_and_the_old_claim_can_no_longer_commit`, and
  the property below.
- **A repeated finish is exact.** A worker that did not see its commit's
  outcome can call finish again and gets the original outcome, with no new
  revision, even though the job is no longer claimed.
  `a_repeated_finish_returns_the_original_outcome_after_the_job_moved_on`.
- **A dead worker's job is finished by another process.**
  `a_killed_workers_job_is_finished_by_another_process` kills a worker
  process that holds a claim. Two more processes, whose clocks run a day
  ahead so that its lease has passed for them, take the job and finish it.
- **Every state is kept.** `history_keeps_every_state_a_job_passed_through`.
- **Sequences of these operations keep the rules above.**
  `random_traces_match_the_claim_state_machine` runs 1,024 random sequential
  traces of enqueues, claims, heartbeats, finishes, releases, failures and
  clock steps by three workers, who keep and reuse every claim they were ever
  given. It compares each outcome, and the stored state at the end, with a
  small state machine. The traces are sequential: concurrency is covered by
  the process tests and by Factlite's own commit serialization.

Sixteen single changes to the pattern were each tried by hand. Fourteen made
at least one test fail: removing the owner, attempt or lease expectation from
a holder's write; removing the attempt expectation from a claim; removing the
lease expectation from a takeover; keeping the due time on a claim or the
lease on a finish; letting a heartbeat shorten a lease; removing the check
that a candidate is claimable at the claimer's time; four changes to the
boundaries of "due" and "expired"; and dropping the attempt from the finish
key. Two passed every test: removing the state expectation from a holder's
write, and from a takeover. Both are implied by the lease expectation, since
a job that is not claimed holds no lease. They are kept as a second guard.
This was not done with a mutation tool.

## What is not guaranteed

- **A job's handler can run more than once.** After a takeover the first
  worker may still be working. Only one attempt's finish is recorded, but any
  effect outside the store happens once per attempt that ran. Make outside
  effects idempotent under a key that is the same for every attempt, such as
  the job id and the name of the effect. Do not put the attempt number in
  that key: two attempts would then both take effect. The attempt number is
  useful separately, as a fencing token, where the destination can refuse a
  lower one.
- **Leases compare the workers' own clocks.** On one host that is one clock.
  A worker whose clock runs ahead takes jobs early.
- **Time alone takes nothing away.** A worker whose lease has passed still
  holds the job until another worker takes it, and can still heartbeat and
  finish. The state machine in the tests has this rule.
- **Order is only as strict as `spread`.** With `spread` 1 and `salt` 0 a
  worker asks for the oldest due job, or the oldest expired job when none is
  due. A larger `spread` lets workers choose among the first few of each
  kind.
- **The worker loop is a sketch.** `work_until_stopped` cannot fail or
  release a job, leaves a claim to expire if its handler panics or a commit's
  outcome is unknown, and relies on the producer enqueueing the stop job
  last. Its handling of a busy store is not tested.

## What the store does not do for you

- **No notification.** An idle worker sleeps and reads again. Reading the head
  is cheap, but a job can become claimable with no new revision, when its
  `run-after` or someone's lease passes, so the head alone does not say
  whether to look.
- **No claim-next operation.** A claim is a read and then a commit. Under
  contention some commits lose, and a lost attempt costs a read and a
  refused commit.
- **No claim by job id in one request.** Expectations name entities by id,
  not by lookup ref, so claiming a known `job/id` is a lookup and then a
  commit.
- **No pruning.** See [costs](#costs).

## Costs

Measured on 2026-11-23 on an Apple M4 Pro, macOS 27.2.1, release build, in the
OS temporary directory. Raw output:
[`benchmarks/2026-11-23-macos-work-queue.jsonl`](benchmarks/2026-11-23-macos-work-queue.jsonl).
Every job is enqueued before the workers start, each with its own due time.
Workers do no work between claim and finish. These are one run each on one
machine, not a latency promise. Method and more figures:
[measurements](measurements.md).

| Workers | Jobs | Spread | Jobs per second | Lost claims | File | Writable reopen |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 500 | 1 | 1,292 | 0 | 4.8 MB | 49 ms |
| 1 | 2,000 | 1 | 1,214 | 0 | 19.0 MB | 197 ms |
| 4 | 2,000 | 1 | 1,164 | 220 | 19.0 MB | 193 ms |
| 4 | 2,000 | 8 | 1,099 | 120 | 19.0 MB | 197 ms |
| 8 | 2,000 | 16 | 1,070 | 123 | 19.1 MB | 195 ms |
| 1 | 8,000 | 1 | 1,029 | 0 | 75.8 MB | 817 ms |
| 4 | 8,000 | 8 | 998 | 735 | 76.2 MB | 813 ms |

What these runs show:

- **Each job is three permanent revisions and about 9.5 KB of file.** Enqueue,
  claim and finish; more with heartbeats, releases or takeovers. Nothing can
  be removed, and the file does not shrink.
- **A writable open cost 32 to 34 microseconds per revision:** 0.8 s after
  8,000 jobs. Every worker process pays it at start.
- **A claim slows a little as finished jobs accumulate.** The read that starts
  every claim took 0.22 ms with no finished jobs. With 8,000 finished it took
  0.6 ms, and 1.0 ms once the leases those jobs were worked under had passed:
  roughly 0.05 to 0.1 ms per 1,000 finished jobs.
- **More worker processes did not raise throughput.** Commits are serialized
  by SQLite's lock and these workers do nothing else. Several workers are for
  running handlers at the same time, not for draining faster.

What to do about it:

- **Give a queue its own store file, and start a new file when the old one's
  size or open time matters.** Enqueue to the new file and let workers drain
  the old one. A file is also the only thing that can be deleted.
- **Prefer a long lease to heartbeats.** Each heartbeat is a revision.
- **Pass the enqueue time as `run_after`, not a constant.** Jobs that share
  one due time are read as a whole tie: with 2,000 pending jobs a candidate
  read took 0.22 ms with distinct due times and 2.0 ms with one.

### Why the pattern retracts scheduling values

An earlier form of this pattern left `job/run-after` and `job/lease-until` on
finished jobs. Its claim read took 6.5 ms with 8,000 finished jobs, and one
worker's throughput fell from 1,185 jobs per second over 500 jobs to 258 over
8,000 ([before](benchmarks/2026-11-23-macos-work-queue-before.jsonl)). A
reviewer measured 12.6 ms for the same read once those jobs' leases had
passed, a case the runs above did not include.

The cause is how the two reads are executed. Each is driven from a time
range, due times up to now or leases before now, and then looks up the
queue, state and attempt of every job in that range. Finished jobs have the
oldest times, so every read stepped over all of them before reaching a
pending job. A review reproduced this from the query plan of the generated
SQL on plain SQLite. Once a finished job holds neither value, the range
holds only closed intervals for it, which are skipped from the index without
a lookup. That is the remaining slope.

This is a property of how Factlite executes a query that joins attributes,
not only of queues. It is recorded in the [roadmap](roadmap.md), item 12.

## Permanence

Payloads, results, owner names and idempotency keys are kept for the life of
the store ([permanence](permanence.md)). Put a reference in `job/payload`, not
the material itself, when the material may contain secrets or may need to be
deleted. The [agent guide](agents.md) says more.

## Other interfaces

The CLI and the babashka pod accept the same requests: expectations are the
`expect` entries of a transaction. The queue pattern itself is tested only
through the Rust API. The pod's
[process test](../crates/factlite-pod/tests/bb.rs) covers keyed creates and
conditional writes from several pod processes.
