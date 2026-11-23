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
| `job/id` | string, unique | The producer's name for the job. |
| `job/queue` | keyword | Which queue the job is in. |
| `job/state` | keyword | `pending`, `claimed`, `done` or `failed`. |
| `job/payload` | string | What to do. Permanent; see [permanence](#permanence). |
| `job/run-after` | inst | When the job becomes claimable. |
| `job/attempt` | i64 | Claims made so far. The fence. |
| `job/owner` | string | The last worker to claim the job. |
| `job/lease-until` | inst | When another worker may take the job. |
| `job/result` | string | Set by finish or fail. |

A job moves from `pending` to `claimed` and then to `done` or `failed`. A
claimed job can also go back to `pending` (release), or stay `claimed` under a
new owner and the next attempt (takeover).

Factlite records no clock. Every function takes the current time from its
caller.

## Operations

Each operation is one commit. "Expects" lists the exact values the job must
hold for the commit to be accepted.

| Operation | Expects | Writes | Idempotency key |
| --- | --- | --- | --- |
| `enqueue` | nothing | a new pending job at attempt 0 | from `job/id` |
| `try_claim`, pending job | state `pending`, attempt as read | state `claimed`, owner, lease, attempt + 1 | none |
| `try_claim`, expired job | state `claimed`, lease and attempt as read | owner, lease, attempt + 1 | none |
| `heartbeat` | state `claimed`, owner, attempt | a later lease, never an earlier one | none |
| `finish` | state `claimed`, owner, attempt | state `done`, result | from job and attempt |
| `fail` | state `claimed`, owner, attempt | state `failed`, result | none |
| `release` | state `claimed`, owner, attempt | state `pending`, a new `run-after` | none |

`candidates` reads the jobs claimable at one snapshot: pending jobs that are
due, oldest first, then claimed jobs whose lease has passed. Factlite queries
have no disjunction, so it is two queries against the same view, each with its
own limit. `try_claim` refuses, without committing, a candidate that is not
claimable at the time it is given. `claim` reads candidates, tries one, and
reads again if it lost.

A claim belongs to one thread of control. Two writers working from copies of
one claim are not told apart.

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
  attempt. The attempt is needed as well as the owner: a worker that claims
  its own job again, for example after a restart under the same name, must
  not finish with the earlier claim.
  `an_expired_lease_is_taken_and_the_old_claim_can_no_longer_commit`,
  `a_worker_that_claims_its_own_expired_job_again_loses_the_earlier_claim`.
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

Eight single changes to the pattern were each tried by hand and each made at
least one test fail: removing the state, owner or attempt expectation from a
holder's write; removing the attempt expectation from a claim; removing the
state or lease expectation from a takeover; letting a heartbeat shorten a
lease; and removing the check that a candidate is claimable at the claimer's
time. This was not done with a mutation tool.

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
  last.

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
Every job is enqueued before the workers start. These are one run each on one
machine, not a latency promise.

| Workers | Jobs | Spread | Jobs per second | Lost claims | File | Writable reopen |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 500 | 1 | 1,185 | 0 | 4.4 MB | 43 ms |
| 1 | 2,000 | 1 | 741 | 0 | 17.5 MB | 184 ms |
| 4 | 2,000 | 1 | 655 | 2,300 | 17.5 MB | 183 ms |
| 4 | 2,000 | 8 | 986 | 256 | 17.4 MB | 185 ms |
| 8 | 2,000 | 16 | 959 | 228 | 17.6 MB | 185 ms |
| 1 | 8,000 | 1 | 258 | 0 | 69.8 MB | 776 ms |
| 4 | 8,000 | 8 | 551 | 2,165 | 69.8 MB | 951 ms |

What these runs show:

- **Each job is three permanent revisions and about 8.7 KB of file.** Enqueue,
  claim and finish; more with heartbeats, releases or takeovers. Nothing can
  be removed, and the file does not shrink.
- **A claim slows down as finished jobs accumulate.** The read that starts
  every claim took 0.22 ms with no finished jobs in the file, 1.5 ms with
  2,000, and 6.6 ms and 9.8 ms in the two runs with 8,000: about 0.8 to 1.2
  ms per 1,000 finished jobs. It did not depend on the number of pending
  jobs. Throughput with one worker was 1,185 jobs per second over 500 jobs
  and 258 over 8,000.
- **A writable open cost about 31 to 40 microseconds per revision:** 0.78 s
  and 0.95 s after 8,000 jobs. Every worker process pays it at start.
- **In these runs more workers helped only with a spread.** Commits are
  serialized by SQLite's lock. With `spread` 1, four workers were slower
  than one and lost 2,300 claims. With `spread` 8 they were faster.

What to do about it:

- **Give a queue its own store file, and start a new file after a few
  thousand jobs.** Enqueue to the new file and let workers drain the old one.
  A file is also the only thing that can be deleted.
- **Prefer a long lease to heartbeats.** Each heartbeat is a revision.
- **Set `spread` to a few times the worker count** unless strict order
  matters.

Why the claim read slows down: its query joins four attributes of a job, and
finished jobs still hold three of them. Further measurements, recorded in
[measurements](measurements.md), show that any read joining two or more of
these attributes grew at about the same rate, while a read of the single
pattern `job/state = pending` grew about fourteen times more slowly, 0.46 ms
against 6.5 ms at 8,000 finished jobs. So the cost comes from the join being
driven from an attribute that every job holds, not from finding the pending
jobs. A reviewer's reproduction of the generated SQL on plain SQLite found
the same: the query was driven from the due-time range and probed state per
job.

The pattern does not use the single-pattern read, because that read cannot
order by due time or bound its result in the query. A queue that needs
flatter claim times could read the pending set that way and pull each
candidate, at about 0.03 ms per pull; that variant is not implemented or
tested here. A remedy in the library, in how a query's joins are ordered or
indexed, needs its own decision and is recorded in the
[roadmap](roadmap.md), item 12.

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
