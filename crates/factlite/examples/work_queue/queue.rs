//! A durable work queue written against Factlite's public API.
//!
//! The `work_queue` example and `tests/work_queue.rs` both include this file,
//! so the guide in `docs/queues.md`, the example and the tests use the same
//! code. Nothing here is part of the library.
//!
//! Every function takes the current time from the caller, in milliseconds
//! since the Unix epoch. Factlite records no clock. Lease lengths must be
//! positive.
//!
//! Copies of a [`Claim`] are interchangeable: any copy at the current attempt
//! may write, so a heartbeat thread can run beside the thread doing the work.
//! They are not told apart.

use factlite::{
    AttrName, AttributeDefinition, Cardinality, CommitOutcome, Comparison, Db, Direction, EntityId,
    ErrorCode, Keyword, Order, Params, Pattern, Predicate, Query, QueryName, Result, Store, TempId,
    Term, Tx, TxBuilder, Uniqueness, Value, ValueType,
};

/// Waiting to be claimed.
pub const PENDING: &str = "pending";
/// Held by the worker in `job/owner` until `job/lease-until`.
pub const CLAIMED: &str = "claimed";
/// Finished. Never changes again.
pub const DONE: &str = "done";
/// Given up on. Never changes again.
pub const FAILED: &str = "failed";

/// The due time of a job that is not pending, and the lease of a job that was
/// claimed and no longer is: an instant no clock reaches.
///
/// The two reads in [`candidates`] are each driven from a time range, so a
/// finished job must not hold a time inside it. Writing this value replaces
/// whatever was stored without having to name it, which a retraction would.
pub const NEVER: i64 = i64::MAX;

/// Qualified attribute name. Panics on an invalid literal.
pub fn attr(name: &str) -> AttrName {
    name.parse().expect("valid attribute name")
}

fn var(name: &str) -> QueryName {
    QueryName::new(name).expect("valid query name")
}

fn keyword(text: &str) -> Value {
    Value::Keyword(Keyword::new(text).expect("valid keyword"))
}

fn string(text: &str) -> Value {
    Value::String(text.into())
}

/// A worker's hold on one job. `attempt` is the fence: every later write by
/// this worker expects it, so a worker whose lease was taken cannot commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Claim {
    /// The job entity.
    pub job: EntityId,
    /// The attempt number this claim created, starting at 1.
    pub attempt: i64,
    /// The instant after which another worker may take the job.
    pub lease_until: i64,
}

/// A job that looked claimable at one snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Candidate {
    /// The job entity.
    pub job: EntityId,
    /// Attempts made so far, as read at the snapshot.
    pub attempt: i64,
    /// Why the job was claimable.
    pub basis: Basis,
}

/// Why a [`Candidate`] was claimable, with the value that made it so.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Basis {
    /// Pending, and due at `run_after`.
    Due {
        /// The job's `job/run-after`.
        run_after: i64,
    },
    /// Claimed, under a lease that ended at `lease_until`.
    Expired {
        /// The job's `job/lease-until`.
        lease_until: i64,
    },
}

impl Basis {
    /// Whether a job read with this basis is claimable at `now`.
    pub fn holds_at(self, now: i64) -> bool {
        match self {
            Self::Due { run_after } => run_after <= now,
            Self::Expired { lease_until } => lease_until < now,
        }
    }
}

/// Declares the queue's attributes. Safe to call from every process: the
/// idempotency key makes later calls return the first call's report.
pub fn define_schema(store: &Store) -> Result<()> {
    let mut tx = Tx::builder().idempotency_key("queue/schema/v1");
    for (name, ty, unique) in [
        ("job/id", ValueType::String, Uniqueness::UniqueOne),
        ("job/queue", ValueType::Keyword, Uniqueness::None),
        ("job/state", ValueType::Keyword, Uniqueness::None),
        ("job/payload", ValueType::String, Uniqueness::None),
        ("job/run-after", ValueType::Inst, Uniqueness::None),
        ("job/attempt", ValueType::I64, Uniqueness::None),
        ("job/owner", ValueType::String, Uniqueness::None),
        ("job/lease-until", ValueType::Inst, Uniqueness::None),
        ("job/result", ValueType::String, Uniqueness::None),
    ] {
        tx = tx.define_attribute(AttributeDefinition::new(
            attr(name),
            ty,
            Cardinality::One,
            unique,
        )?);
    }
    store.commit(tx.build()?)?;
    Ok(())
}

/// Adds a pending job that becomes claimable at `run_after`.
///
/// The idempotency key is derived from `id`, so repeating the same call
/// returns the same entity and commits nothing. The same `id` with different
/// contents fails with `IdempotencyConflict`.
pub fn enqueue(
    store: &Store,
    queue: &str,
    id: &str,
    payload: &str,
    run_after: i64,
) -> Result<EntityId> {
    let job = TempId::new("job")?;
    let report = store.commit(
        Tx::builder()
            .idempotency_key(format!("queue/enqueue/{id}"))
            .new_entity(job.clone())
            .assert(job.clone(), attr("job/id"), string(id))
            .assert(job.clone(), attr("job/queue"), keyword(queue))
            .assert(job.clone(), attr("job/state"), keyword(PENDING))
            .assert(job.clone(), attr("job/payload"), string(payload))
            .assert(job.clone(), attr("job/run-after"), Value::Inst(run_after))
            .assert(job.clone(), attr("job/attempt"), Value::I64(0))
            .build()?,
    )?;
    Ok(report.tempids[&job].clone())
}

/// Jobs in `queue` that are claimable at `now` in this view: pending jobs
/// that are due, oldest `run-after` first, then claimed jobs whose lease has
/// passed, oldest lease first. At most `limit` of each kind, so up to twice
/// `limit` in all.
///
/// Factlite queries have no disjunction, so this is two queries. Both read the
/// same view, so they agree with each other.
pub fn candidates(db: &Db, queue: &str, now: i64, limit: u64) -> Result<Vec<Candidate>> {
    let scan = |state: &str, time: &str, comparison: Comparison| {
        let query = Query {
            inputs: [
                (var("queue"), ValueType::Keyword),
                (var("state"), ValueType::Keyword),
                (var("now"), ValueType::Inst),
            ]
            .into(),
            patterns: [
                ("job/queue", Term::Param(var("queue"))),
                ("job/state", Term::Param(var("state"))),
                (time, Term::Var(var("time"))),
                ("job/attempt", Term::Var(var("attempt"))),
            ]
            .into_iter()
            .map(|(attribute, value)| Pattern {
                entity: Term::Var(var("job")),
                attribute: attr(attribute),
                value,
            })
            .collect(),
            predicates: vec![Predicate {
                left: Term::Var(var("time")),
                comparison,
                right: Term::Param(var("now")),
            }],
            select: vec![var("job"), var("time"), var("attempt")],
            order: ["time", "job"]
                .into_iter()
                .map(|field| Order {
                    field: var(field),
                    direction: Direction::Asc,
                })
                .collect(),
            limit: Some(limit),
            ..Query::default()
        };
        let params: Params = [
            (var("queue"), keyword(queue)),
            (var("state"), keyword(state)),
            (var("now"), Value::Inst(now)),
        ]
        .into();
        db.query(&query, &params)
    };
    let mut found = Vec::new();
    for (state, time, comparison) in [
        (PENDING, "job/run-after", Comparison::Le),
        (CLAIMED, "job/lease-until", Comparison::Lt),
    ] {
        for row in scan(state, time, comparison)?.rows {
            let [Value::Ref(job), Value::Inst(at), Value::I64(attempt)] = &row[..] else {
                unreachable!("the selected columns are a job, an instant and an attempt");
            };
            found.push(Candidate {
                job: job.clone(),
                attempt: *attempt,
                basis: match state {
                    PENDING => Basis::Due { run_after: *at },
                    _ => Basis::Expired { lease_until: *at },
                },
            });
        }
    }
    Ok(found)
}

/// Tries to take one candidate. Returns `None` if the job changed after the
/// candidate was read: another worker claimed it, its owner extended the
/// lease, or its owner finished, failed or released it. Also returns `None`,
/// without committing, if the candidate is not claimable at this `now`.
///
/// The expectations name the exact values the candidate was read with. A
/// pending job must still be pending at the same attempt. An expired job must
/// still be claimed, at the same attempt, under the same lease. The lease
/// expectation makes a concurrent heartbeat win. It also makes a late finish,
/// fail or release win, because each of those sets the lease to [`NEVER`];
/// the state expectation is a second guard for the same case.
pub fn try_claim(
    store: &Store,
    candidate: &Candidate,
    worker: &str,
    now: i64,
    lease_ms: i64,
) -> Result<Option<Claim>> {
    if !candidate.basis.holds_at(now) {
        return Ok(None);
    }
    commit_claim(store, candidate, worker, now, lease_ms)
}

/// Commits a claim of `candidate` without checking it against `now`.
///
/// The claim of a pending job sets its due time to [`NEVER`].
fn commit_claim(
    store: &Store,
    candidate: &Candidate,
    worker: &str,
    now: i64,
    lease_ms: i64,
) -> Result<Option<Claim>> {
    let job = &candidate.job;
    let claim = Claim {
        job: job.clone(),
        attempt: candidate.attempt + 1,
        lease_until: now.saturating_add(lease_ms).min(NEVER - 1),
    };
    let mut tx = Tx::builder().expect(
        job.clone(),
        attr("job/attempt"),
        vec![Value::I64(candidate.attempt)],
    );
    tx = match candidate.basis {
        Basis::Due { .. } => tx
            .expect(job.clone(), attr("job/state"), vec![keyword(PENDING)])
            .assert(job.clone(), attr("job/run-after"), Value::Inst(NEVER)),
        Basis::Expired { lease_until } => tx
            .expect(job.clone(), attr("job/state"), vec![keyword(CLAIMED)])
            .expect(
                job.clone(),
                attr("job/lease-until"),
                vec![Value::Inst(lease_until)],
            ),
    };
    let tx = tx
        .assert(job.clone(), attr("job/state"), keyword(CLAIMED))
        .assert(job.clone(), attr("job/owner"), string(worker))
        .assert(job.clone(), attr("job/attempt"), Value::I64(claim.attempt))
        .assert(
            job.clone(),
            attr("job/lease-until"),
            Value::Inst(claim.lease_until),
        );
    Ok(commit_unless_changed(store, tx)?.then_some(claim))
}

/// The outcome of [`claim`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Claimed {
    /// The claim, or `None` when nothing was claimable.
    pub claim: Option<Claim>,
    /// Candidates this call lost to another worker before it returned.
    pub lost: u64,
}

/// Claims one job from `queue`, or returns no claim when none is claimable at
/// `now`.
///
/// Reads up to `spread` candidates of each kind, due jobs before expired
/// ones, and starts from the one `salt` selects. With `spread` 1 and `salt`
/// 0 every worker asks for the oldest due job, or for the oldest expired job
/// when none is due. A larger spread and a salt that differs between workers
/// trades strict ordering for fewer lost attempts. After a lost attempt it
/// reads again.
pub fn claim(
    store: &Store,
    queue: &str,
    worker: &str,
    now: i64,
    lease_ms: i64,
    spread: u64,
    salt: u64,
) -> Result<Claimed> {
    let mut lost = 0;
    loop {
        let found = candidates(&store.db()?, queue, now, spread)?;
        if found.is_empty() {
            return Ok(Claimed { claim: None, lost });
        }
        let index = usize::try_from(salt.wrapping_add(lost) % found.len() as u64)
            .expect("an index below the candidate count");
        // The candidates were read for this `now`, so only a change to the
        // job can refuse the claim, and each refusal means another commit.
        if let Some(claim) = commit_claim(store, &found[index], worker, now, lease_ms)? {
            return Ok(Claimed {
                claim: Some(claim),
                lost,
            });
        }
        lost += 1;
    }
}

/// Starts a request that commits only while `worker` still holds `claim`:
/// the job is claimed, by this worker, at this attempt. The lease is not
/// part of the fence, so every copy of a claim stays usable after a
/// heartbeat.
fn fenced(claim: &Claim, worker: &str) -> TxBuilder {
    Tx::builder()
        .expect(claim.job.clone(), attr("job/state"), vec![keyword(CLAIMED)])
        .expect(claim.job.clone(), attr("job/owner"), vec![string(worker)])
        .expect(
            claim.job.clone(),
            attr("job/attempt"),
            vec![Value::I64(claim.attempt)],
        )
}

/// Starts a fenced request that gives the job up, ending its lease.
fn giving_up(claim: &Claim, worker: &str) -> TxBuilder {
    fenced(claim, worker).assert(
        claim.job.clone(),
        attr("job/lease-until"),
        Value::Inst(NEVER),
    )
}

/// Commits, mapping a failed expectation to `false`.
fn commit_unless_changed(store: &Store, tx: TxBuilder) -> Result<bool> {
    match store.commit(tx.build()?) {
        Ok(_) => Ok(true),
        Err(error) if error.code() == ErrorCode::PreconditionFailed => Ok(false),
        Err(error) => Err(error),
    }
}

/// Extends the lease to `now + lease_ms`, never shortening the lease this
/// copy of the claim records. Returns `None` if the claim was lost; the
/// worker must then stop work on the job.
///
/// Each heartbeat is a permanent revision. Prefer a lease long enough to need
/// few of them.
pub fn heartbeat(
    store: &Store,
    claim: &Claim,
    worker: &str,
    now: i64,
    lease_ms: i64,
) -> Result<Option<Claim>> {
    let extended = Claim {
        lease_until: claim
            .lease_until
            .max(now.saturating_add(lease_ms))
            .min(NEVER - 1),
        ..claim.clone()
    };
    let tx = fenced(claim, worker).assert(
        claim.job.clone(),
        attr("job/lease-until"),
        Value::Inst(extended.lease_until),
    );
    Ok(commit_unless_changed(store, tx)?.then_some(extended))
}

/// Marks the job done with `result`. Returns `false` if the claim was lost.
///
/// The idempotency key names the job and attempt, so a worker that did not
/// see the outcome of its commit can repeat the call and get the original
/// report, even though the job is no longer claimed.
pub fn finish(store: &Store, claim: &Claim, worker: &str, result: &str) -> Result<bool> {
    let tx = giving_up(claim, worker)
        .idempotency_key(format!("queue/finish/{}/{}", claim.job, claim.attempt))
        .assert(claim.job.clone(), attr("job/state"), keyword(DONE))
        .assert(claim.job.clone(), attr("job/result"), string(result));
    commit_unless_changed(store, tx)
}

/// Gives up on the job with `error` as its result. Returns `false` if the
/// claim was lost.
pub fn fail(store: &Store, claim: &Claim, worker: &str, error: &str) -> Result<bool> {
    let tx = giving_up(claim, worker)
        .assert(claim.job.clone(), attr("job/state"), keyword(FAILED))
        .assert(claim.job.clone(), attr("job/result"), string(error));
    commit_unless_changed(store, tx)
}

/// Returns the job to pending, claimable again at `run_after`. The attempt
/// count is kept. Returns `false` if the claim was lost.
pub fn release(store: &Store, claim: &Claim, worker: &str, run_after: i64) -> Result<bool> {
    let tx = giving_up(claim, worker)
        .assert(claim.job.clone(), attr("job/state"), keyword(PENDING))
        .assert(
            claim.job.clone(),
            attr("job/run-after"),
            Value::Inst(run_after),
        );
    commit_unless_changed(store, tx)
}

/// What one run of [`work_until_stopped`] did.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Worked {
    /// The `job/id` of every job this worker finished, in order.
    pub finished: Vec<String>,
    /// Claim attempts lost to other workers.
    pub lost: u64,
}

/// A minimal worker loop: claim, handle, finish, and poll when idle.
///
/// Returns once the job `stop_id` exists and no job in `queue` is pending or
/// claimed. `now` supplies the clock and `handle` turns a job's id and payload
/// into its result.
///
/// What this loop leaves out:
///
/// - The producer must enqueue the `stop_id` job after its last job. A job
///   enqueued after a worker saw the queue empty and stopped is not worked.
/// - `handle` cannot fail or release a job. If it panics, or this function
///   returns an error, the claim is left to expire and another worker takes
///   the job.
/// - A busy store is retried when nothing was committed. A commit whose
///   outcome is unknown is returned as an error. A claim is not keyed, so a
///   claim lost that way is also left to expire.
///
/// Factlite has no notification, so an idle worker sleeps for `idle` and reads
/// again. A job can become claimable without a new revision, when its
/// `run-after` or another worker's lease passes, so watching the head alone is
/// not enough.
#[allow(clippy::too_many_arguments)]
pub fn work_until_stopped(
    store: &Store,
    queue: &str,
    stop_id: &str,
    worker: &str,
    lease_ms: i64,
    spread: u64,
    idle: std::time::Duration,
    now: impl Fn() -> i64,
    mut handle: impl FnMut(&str, &str) -> String,
) -> Result<Worked> {
    let mut worked = Worked::default();
    let mut salt = worker
        .bytes()
        .fold(0_u64, |sum, byte| sum + u64::from(byte));
    loop {
        salt = salt.wrapping_add(1);
        let claimed = retry_busy(|| claim(store, queue, worker, now(), lease_ms, spread, salt))?;
        worked.lost += claimed.lost;
        let Some(claim) = claimed.claim else {
            let stopped = retry_busy(|| {
                let db = store.db()?;
                let stop = db.lookup(&attr("job/id"), &string(stop_id))?;
                Ok(stop.is_some() && !unfinished(&db, queue)?)
            })?;
            if stopped {
                return Ok(worked);
            }
            std::thread::sleep(idle);
            continue;
        };
        let job = retry_busy(|| {
            store
                .db()?
                .pull(&claim.job, &[attr("job/id"), attr("job/payload")])
        })?
        .expect("a claimed job exists");
        let text = |name: &str| match &job.attributes[&attr(name)] {
            factlite::PulledValue::One(Value::String(text)) => text.clone(),
            other => unreachable!("{name} is a single string, not {other:?}"),
        };
        let result = handle(&text("job/id"), &text("job/payload"));
        // False means the lease passed and another worker took the job.
        if retry_busy(|| finish(store, &claim, worker, &result))? {
            worked.finished.push(text("job/id"));
        }
    }
}

/// Repeats `operation` while the store is busy and nothing was committed.
fn retry_busy<T>(mut operation: impl FnMut() -> Result<T>) -> Result<T> {
    loop {
        match operation() {
            Err(error)
                if error.code() == ErrorCode::Busy
                    && error.commit_outcome() != Some(CommitOutcome::Unknown) =>
            {
                std::thread::yield_now();
            }
            other => return other,
        }
    }
}

/// Whether any job in `queue` is pending or claimed in this view.
pub fn unfinished(db: &Db, queue: &str) -> Result<bool> {
    for state in [PENDING, CLAIMED] {
        let query = Query {
            patterns: [("job/queue", queue), ("job/state", state)]
                .into_iter()
                .map(|(attribute, value)| Pattern {
                    entity: Term::Var(var("job")),
                    attribute: attr(attribute),
                    value: Term::Const(keyword(value)),
                })
                .collect(),
            select: vec![var("job")],
            limit: Some(1),
            ..Query::default()
        };
        if !db.query(&query, &Params::default())?.rows.is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}
