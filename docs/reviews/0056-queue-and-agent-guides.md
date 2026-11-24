# Review 0056: work queue guide and agent guide

Branch `docs/queue-and-agent-guides`, PR #21. Plan items 13.1 and 13.2.
Examples, tests and documentation; no library code changed. No human reviewer
ran. GitHub Actions did not run: every job on this branch, and a re-run made
after the user reported the account fixed, was refused before starting, with
the message that the account's payments failed or its spending limit needs
raising.

## Method

- **Self-review** of the complete diff, with hand mutations of the pattern
  and repeated runs of the process tests.
- **Codex subagent**, through delega, read-only, high effort, on commit
  `4cec060`. It reproduced the query plan on in-memory SQLite 3.45.3 and did
  not run the Rust tests.
- **Claude subagent**, read-only, started on `4cec060` and finished on
  `efdac31` after the Codex fixes landed. It ran the queue tests and both
  examples, timed variants in a scratch crate outside the repository, and
  read query plans on system SQLite 3.54.0.
- **Claude subagent, second pass,** on the changes made after both reviews
  (`efdac31..22fc2bf`). It reran the sixteen mutations and the timings.
- **Not independently reviewed:** the last change, which replaced
  retractions with a "never" value in response to the second pass.

## Self-review findings

- **The attempt fence was untested.** Removing the attempt expectation from a
  holder's write passed the first eleven tests, because the property kept
  only each worker's latest claim per job. Added a deterministic case and
  made the property keep every claim.
- **The first benchmark gave every job one due time,** which made the claim
  read grow with pending jobs. Jobs now get distinct due times, and the guide
  says to pass the enqueue time.
- **The claim read grew with finished jobs.** Measured, traced to joined reads
  being driven from an attribute every job held, and recorded as roadmap 12.
  Later removed from the pattern; see the second review's finding 3.

## Codex findings

1. **The guide advised keying outside effects by job and attempt.** Two
   attempts would then both take effect. Adopted: key by something stable
   across attempts; the attempt is a fencing token.
2. **A heartbeat could shorten a lease, and `try_claim` trusted its
   candidate.** Adopted: a heartbeat takes the later of the held and
   requested lease; a candidate records why it was claimable and is refused
   at a time when it is not; deadline arithmetic saturates. The suggested
   compare-and-set on the prior lease was tried with the retractions and
   then dropped; see the second pass. Lease lengths are documented as
   positive, not validated.
3. **The worker loop handled `Busy` inconsistently and could abandon a
   claim.** Partly adopted: every store call in the loop retries a busy store
   when nothing was committed, and an unknown outcome is returned as an
   error. Not adopted: a way for a handler to fail or release a job, and
   recovery of a claim whose commit outcome is unknown. The loop's doc and
   the guide list both as left out.
4. **The stop condition does not close the queue.** Documented as the
   producer's contract; not changed.
5. **The property's model mispredicted a repeated enqueue after a release,
   accepted a broken finish replay, and copied the heartbeat's deadline.**
   Adopted, all three. The second review measured the first as a spurious
   failure in about one 1,024-case run in eight.
6. **The process tests did not force contention, the kill test used a 300 ms
   lease, and a takeover after a late finish was untested.** Adopted: a test
   in which four processes all hold one candidate before any may claim; the
   kill test driven by a clock offset; deterministic cases for a takeover
   after finish, fail and release; a case for due jobs before expired ones.
7. **The slowdown's cause was the join being driven from the due-time
   range.** Adopted into the guide and roadmap 12. The suggested generation
   marker was not adopted.
8. **Some conclusions exceeded the benchmark.** Adopted: figures are scoped to
   the runs made. Not adopted: a readiness barrier in the measured run, which
   still starts workers on a wall-clock target two seconds ahead, and a
   monotonic drain timer. The lost-claim count can undercount; the
   measurements say so.
9. **The agent guide overstated what its example checks.** Adopted: the
   schema commit is described as unstamped shared setup, the handoff claim
   cites the library tests that show it, and key encoding is addressed.

## Claude findings

1. **The published claim-read cost left out the takeover scan,** which about
   doubles it once finished jobs' leases have passed. Adopted: the measured
   run reports a third read at a time past every lease.
2. **`status.md` cited this file before it existed.** This file.
3. **The slowdown explanation missed the mechanism and a remedy inside the
   pattern:** finished jobs hold the oldest times, so each read stepped over
   them, and a closed interval is skipped from the index alone. Adopted,
   first as retractions and finally as the reviewer first proposed it: a
   claim sets the due time to the largest instant; finish, fail and release
   set the lease to it. Over 8,000 jobs the claim read went from 6.5 ms to
   0.6 ms and one worker from 258 to 925 jobs per second.
4. **`claim` could loop forever if the query and the time check disagreed.**
   Two boundary mutations hung the suite. Adopted: `claim` commits without
   rechecking the time. The same mutations now fail tests.
5. **Test gaps.** Adopted: a stale finish after the job is done; the original
   enqueue after a reschedule; a small measured run in the gate. Not adopted:
   a test of `retry_busy`; the guide says it is untested. The state
   expectation on a pending claim is redundant and kept.
6. **Jobs sharing one due time were not in the guide.** Adopted.
7. **Three agent guide claims were stronger than the evidence:** the open
   cost range, a statement about other stores, and when a key conflicts.
   Adopted, all three.
8. **Smaller points.** Adopted: slopes and open cost restated from the new
   runs; "nothing is retracted" removed; the two new `status.md` headings use
   a colon; a `testing.md` entry; `job/id` documented as store-wide; the
   behaviour of a repeated `fail` or `release` documented.

## Claude second-pass findings

On the retraction form at `22fc2bf`. It found no broken guarantee, confirmed
the invariant on every path and that `claim` terminates, and reproduced the
published figures and the mutation results.

1. **With the lease in the fence, a worker could be refused while it still
   held the job:** a copy of a claim from before a heartbeat, a heartbeat
   thread beside a working thread, or a heartbeat whose outcome the caller
   did not see. A finish from such a copy returned `false` and dropped its
   result. Adopted by removing the cause: the two values are now set to the
   largest instant instead of retracted, so no write has to name the stored
   lease, the lease left the fence, and copies of a claim are
   interchangeable. A test finishes with the earlier copy and repeats the
   finish with the later one.
2. **Three statements described the old mechanism.** Corrected in the code
   comment, the test comment and the guide.
3. **A claim's retraction of the due time was exact by argument only.** No
   longer applies: a claim writes a value.
4. **Small gaps.** Adopted: `fail` is in the scheduling-values test; the
   redundant state expectation on a pending claim is named in the guide; the
   measurements say when the workers start.


## Mutations

Sixteen single changes to `queue.rs` were applied by hand to the final form.
Fourteen fail at least one test. Two pass: removing the state expectation
from a takeover, which the lease expectation implies, and from a pending
claim, which the attempt expectation implies.

## Validation

`REQUIRE_BB=1 ./scripts/check.sh` passes locally on the final form with 247
tests,
including the repository link test, both new examples and a three-process
measured run. The three process tests passed repeated runs. External links
are not checked by any test.

## Limitations

- The final change to the pattern was not independently reviewed. It is
  covered by the same tests and mutations.
- The queue pattern is tested through the Rust API only, not the CLI or pod.
- The property's traces are sequential. Concurrency is covered by three
  process tests and by the library's own commit serialization.
- `retry_busy`, an unknown commit outcome and a real busy store are untested
  in the worker loop.
- Measurements are one run per configuration on one machine, up to 8,000
  jobs and 24,002 revisions.
- The query plan inside the bundled SQLite was not captured. The plans cited
  come from the same SQL on other SQLite builds.
- The read-variant table and the extra-index experiment in `measurements.md`
  came from tests that are not in the repository.
- `agent_session` is an example with assertions, not a property test.

## Addendum, 2026-11-23

Written after the review above and before the merge. The user made the
repository public, and GitHub Actions then ran. The `check` job passed on the
branch head `26fb73f` with 247 tests on ubuntu-latest. The statements above
that GitHub Actions did not run describe the state when the review was
written. PR #21 was merged after that run.
