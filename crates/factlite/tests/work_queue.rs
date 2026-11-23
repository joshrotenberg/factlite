//! The work queue pattern in `examples/work_queue/queue.rs`: deterministic
//! contracts, a random-trace property against a small state machine, and
//! separate worker processes contending for one file.

#[path = "../examples/work_queue/queue.rs"]
mod queue;

use factlite::{
    Db, EntityId, ErrorCode, FactFilter, Operation, PulledValue, Revision, Store, TempId, Tx, Value,
};
use proptest::prelude::*;
use queue::{CLAIMED, Candidate, Claim, DONE, FAILED, PENDING, attr};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const QUEUE: &str = "work";
const LEASE: i64 = 100;

fn open() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("queue.sqlite")).unwrap();
    queue::define_schema(&store).unwrap();
    (dir, store)
}

fn one(db: &Db, job: &EntityId, name: &str) -> Option<Value> {
    let entity = db.pull(job, &[attr(name)]).unwrap().unwrap();
    entity.attributes.get(&attr(name)).map(|value| match value {
        PulledValue::One(value) => value.clone(),
        PulledValue::Many(_) => unreachable!("queue attributes are cardinality one"),
    })
}

fn state(store: &Store, job: &EntityId) -> String {
    match one(&store.db().unwrap(), job, "job/state") {
        Some(Value::Keyword(keyword)) => keyword.as_str().to_owned(),
        other => panic!("job/state must be a keyword, not {other:?}"),
    }
}

fn claim(store: &Store, worker: &str, now: i64) -> Option<Claim> {
    let claimed = queue::claim(store, QUEUE, worker, now, LEASE, 1, 0).unwrap();
    assert_eq!(claimed.lost, 0, "nothing else is committing");
    claimed.claim
}

#[test]
fn enqueue_is_exact_on_retry_and_refuses_a_changed_or_duplicate_job() {
    let (_dir, store) = open();
    let job = queue::enqueue(&store, QUEUE, "a", "payload", 0).unwrap();
    let head = store.head().unwrap();
    // The same call returns the same entity and commits nothing.
    assert_eq!(
        queue::enqueue(&store, QUEUE, "a", "payload", 0).unwrap(),
        job
    );
    assert_eq!(store.head().unwrap(), head);
    // The schema call is exact too, so every process may make it.
    queue::define_schema(&store).unwrap();
    assert_eq!(store.head().unwrap(), head);
    // The same job id with different contents is a different request.
    let changed = queue::enqueue(&store, QUEUE, "a", "other", 0).unwrap_err();
    assert_eq!(changed.code(), ErrorCode::IdempotencyConflict);
    // Without a key, the unique job id still refuses a second job.
    let label = TempId::new("job").unwrap();
    let duplicate = store
        .commit(
            Tx::builder()
                .new_entity(label.clone())
                .assert(label, attr("job/id"), Value::String("a".into()))
                .build()
                .unwrap(),
        )
        .unwrap_err();
    assert_eq!(duplicate.code(), ErrorCode::UniqueViolation);
    assert_eq!(store.head().unwrap(), head);
}

#[test]
fn due_time_and_order_decide_what_is_claimable() {
    let (_dir, store) = open();
    let late = queue::enqueue(&store, QUEUE, "late", "", 50).unwrap();
    let early = queue::enqueue(&store, QUEUE, "early", "", 10).unwrap();
    queue::enqueue(&store, "other", "elsewhere", "", 0).unwrap();
    assert_eq!(claim(&store, "w", 9), None);
    assert_eq!(claim(&store, "w", 10).unwrap().job, early);
    assert_eq!(claim(&store, "w", 49), None);
    let second = claim(&store, "w", 50).unwrap();
    assert_eq!((second.job.clone(), second.attempt), (late, 1));
    assert_eq!(second.lease_until, 50 + LEASE);
    assert_eq!(claim(&store, "w", 60), None);
    assert!(queue::unfinished(&store.db().unwrap(), QUEUE).unwrap());
    assert!(queue::unfinished(&store.db().unwrap(), "other").unwrap());
    assert!(!queue::unfinished(&store.db().unwrap(), "empty").unwrap());
}

#[test]
fn an_expired_lease_is_taken_and_the_old_claim_can_no_longer_commit() {
    let (_dir, store) = open();
    let job = queue::enqueue(&store, QUEUE, "a", "", 0).unwrap();
    let first = claim(&store, "a", 0).unwrap();
    // The lease holds through its last instant.
    assert_eq!(claim(&store, "b", LEASE), None);
    let second = claim(&store, "b", LEASE + 1).unwrap();
    assert_eq!((second.job.clone(), second.attempt), (job.clone(), 2));
    let head = store.head().unwrap();
    // Every write by the first worker is refused and commits nothing.
    assert_eq!(
        queue::heartbeat(&store, &first, "a", LEASE + 2, LEASE).unwrap(),
        None
    );
    assert!(!queue::finish(&store, &first, "a", "stale").unwrap());
    assert!(!queue::fail(&store, &first, "a", "stale").unwrap());
    assert!(!queue::release(&store, &first, "a", 0).unwrap());
    // Holding the right attempt under the wrong name is refused as well.
    assert!(!queue::finish(&store, &second, "a", "stale").unwrap());
    assert_eq!(store.head().unwrap(), head);
    assert!(queue::finish(&store, &second, "b", "ok").unwrap());
    assert_eq!(state(&store, &job), DONE);
    assert_eq!(
        one(&store.db().unwrap(), &job, "job/result"),
        Some(Value::String("ok".into()))
    );
    // A finished job is never claimable and never changes.
    assert_eq!(claim(&store, "c", i64::MAX - LEASE), None);
    assert!(!queue::release(&store, &second, "b", 0).unwrap());
}

#[test]
fn a_worker_that_claims_its_own_expired_job_again_loses_the_earlier_claim() {
    let (_dir, store) = open();
    queue::enqueue(&store, QUEUE, "a", "", 0).unwrap();
    // The same name, as when a worker restarts and keeps its identity.
    let before = claim(&store, "a", 0).unwrap();
    let after = claim(&store, "a", LEASE + 1).unwrap();
    assert_eq!((before.attempt, after.attempt), (1, 2));
    let head = store.head().unwrap();
    // The owner matches, so only the attempt tells the two claims apart.
    assert_eq!(
        queue::heartbeat(&store, &before, "a", LEASE + 2, LEASE).unwrap(),
        None
    );
    assert!(!queue::finish(&store, &before, "a", "stale").unwrap());
    assert_eq!(store.head().unwrap(), head);
    assert!(queue::finish(&store, &after, "a", "ok").unwrap());
}

#[test]
fn a_heartbeat_between_read_and_reclaim_wins() {
    let (_dir, store) = open();
    queue::enqueue(&store, QUEUE, "a", "", 0).unwrap();
    let held = claim(&store, "a", 0).unwrap();
    let stale = queue::candidates(&store.db().unwrap(), QUEUE, LEASE + 1, 1).unwrap();
    assert_eq!(stale[0].expired_lease, Some(LEASE));
    let extended = queue::heartbeat(&store, &held, "a", LEASE + 1, LEASE)
        .unwrap()
        .unwrap();
    assert_eq!(extended.lease_until, 2 * LEASE + 1);
    let head = store.head().unwrap();
    assert_eq!(
        queue::try_claim(&store, &stale[0], "b", LEASE + 1, LEASE).unwrap(),
        None
    );
    assert_eq!(store.head().unwrap(), head);
    assert!(queue::finish(&store, &extended, "a", "ok").unwrap());
}

#[test]
fn a_stale_pending_candidate_does_not_claim_a_released_job() {
    let (_dir, store) = open();
    let job = queue::enqueue(&store, QUEUE, "a", "", 0).unwrap();
    let stale = queue::candidates(&store.db().unwrap(), QUEUE, 0, 1).unwrap();
    assert_eq!(
        stale,
        [Candidate {
            job: job.clone(),
            attempt: 0,
            expired_lease: None
        }]
    );
    // Claimed and released: pending again, but at attempt 1.
    let held = claim(&store, "a", 0).unwrap();
    assert!(queue::release(&store, &held, "a", 5).unwrap());
    assert_eq!(state(&store, &job), PENDING);
    assert_eq!(
        queue::try_claim(&store, &stale[0], "b", 5, LEASE).unwrap(),
        None
    );
    assert_eq!(claim(&store, "b", 4), None);
    assert_eq!(claim(&store, "b", 5).unwrap().attempt, 2);
}

#[test]
fn a_repeated_finish_returns_the_original_outcome_after_the_job_moved_on() {
    let (_dir, store) = open();
    let job = queue::enqueue(&store, QUEUE, "a", "", 0).unwrap();
    let held = claim(&store, "a", 0).unwrap();
    assert!(queue::finish(&store, &held, "a", "ok").unwrap());
    let head = store.head().unwrap();
    // The job is done, so the expectations no longer hold; the keyed retry
    // still reports the original commit and adds no revision.
    assert!(queue::finish(&store, &held, "a", "ok").unwrap());
    assert_eq!(store.head().unwrap(), head);
    // A different result under the same job and attempt is refused.
    let changed = queue::finish(&store, &held, "a", "different").unwrap_err();
    assert_eq!(changed.code(), ErrorCode::IdempotencyConflict);
    assert_eq!(state(&store, &job), DONE);
}

#[test]
fn history_keeps_every_state_a_job_passed_through() {
    let (_dir, store) = open();
    let job = queue::enqueue(&store, QUEUE, "a", "", 0).unwrap();
    let first = claim(&store, "a", 0).unwrap();
    assert!(queue::release(&store, &first, "a", 0).unwrap());
    let second = claim(&store, "b", 0).unwrap();
    assert!(queue::fail(&store, &second, "b", "gave up").unwrap());
    assert_eq!(state(&store, &job), FAILED);
    let filter = |name: &str| FactFilter {
        entity: Some(job.clone()),
        attribute: Some(attr(name)),
        value: None,
    };
    let asserted = |name: &str| -> Vec<Value> {
        store
            .db()
            .unwrap()
            .history(filter(name), Revision::ZERO)
            .unwrap()
            .into_iter()
            .filter(|event| event.operation == Operation::Assert)
            .map(|event| event.fact.value)
            .collect()
    };
    let keywords: Vec<String> = asserted("job/state")
        .into_iter()
        .map(|value| match value {
            Value::Keyword(keyword) => keyword.as_str().to_owned(),
            other => panic!("unexpected state {other:?}"),
        })
        .collect();
    assert_eq!(keywords, [PENDING, CLAIMED, PENDING, CLAIMED, FAILED]);
    assert_eq!(
        asserted("job/owner"),
        [Value::String("a".into()), Value::String("b".into())]
    );
    assert_eq!(
        asserted("job/attempt"),
        [Value::I64(0), Value::I64(1), Value::I64(2)]
    );
}

/// The documented contract as a state machine, with no queries or storage.
#[derive(Clone, Debug)]
struct ModelJob {
    entity: EntityId,
    state: &'static str,
    owner: Option<usize>,
    attempt: i64,
    lease_until: i64,
    run_after: i64,
}

impl ModelJob {
    fn claimable(&self, now: i64) -> bool {
        (self.state == PENDING && self.run_after <= now)
            || (self.state == CLAIMED && self.lease_until < now)
    }
    fn held_by(&self, worker: usize, claim: &Claim) -> bool {
        self.state == CLAIMED && self.owner == Some(worker) && self.attempt == claim.attempt
    }
}

const WORKERS: [&str; 3] = ["w0", "w1", "w2"];

/// Runs random enqueues, claims, heartbeats, finishes, releases, failures and
/// clock steps. Workers keep every claim they were ever given, including ones
/// they have since lost or replaced by claiming the same job again, and keep
/// trying to use them.
fn run_trace(operations: &[(u8, usize, usize, i64)]) {
    let (_dir, store) = open();
    let mut now = 0_i64;
    let mut jobs: BTreeMap<usize, ModelJob> = BTreeMap::new();
    // Every claim each worker was given for each job, stale or not.
    let mut held: BTreeMap<(usize, usize), Vec<Claim>> = BTreeMap::new();
    for &(kind, worker, job, step) in operations {
        let name = WORKERS[worker];
        match kind {
            0 => {
                let run_after = now + step % 20;
                let entity = queue::enqueue(&store, QUEUE, &format!("job-{job}"), "", run_after);
                match jobs.get(&job) {
                    // A different due time under the same job id is refused.
                    Some(existing) if existing.run_after != run_after => {
                        assert_eq!(entity.unwrap_err().code(), ErrorCode::IdempotencyConflict)
                    }
                    Some(existing) => assert_eq!(entity.unwrap(), existing.entity),
                    None => {
                        jobs.insert(
                            job,
                            ModelJob {
                                entity: entity.unwrap(),
                                state: PENDING,
                                owner: None,
                                attempt: 0,
                                lease_until: 0,
                                run_after,
                            },
                        );
                    }
                }
            }
            1 | 2 => {
                let salt = u64::try_from(step).unwrap();
                let claimed = queue::claim(&store, QUEUE, name, now, LEASE, 4, salt).unwrap();
                assert_eq!(claimed.lost, 0);
                match claimed.claim {
                    None => assert!(!jobs.values().any(|job| job.claimable(now))),
                    Some(claim) => {
                        let (&index, model) = jobs
                            .iter_mut()
                            .find(|(_, job)| job.entity == claim.job)
                            .expect("a claim names an enqueued job");
                        assert!(model.claimable(now), "claimed {model:?} at {now}");
                        assert_eq!(claim.attempt, model.attempt + 1);
                        assert_eq!(claim.lease_until, now + LEASE);
                        model.state = CLAIMED;
                        model.owner = Some(worker);
                        model.attempt = claim.attempt;
                        model.lease_until = claim.lease_until;
                        held.entry((worker, index)).or_default().push(claim);
                    }
                }
            }
            3..=6 => {
                let (Some(claims), Some(model)) =
                    (held.get_mut(&(worker, job)), jobs.get_mut(&job))
                else {
                    continue;
                };
                let chosen = usize::try_from(step).unwrap() % claims.len();
                let claim = &claims[chosen].clone();
                // Only the current owner at the current attempt may write.
                // Time alone takes nothing away: an expired lease that nobody
                // has taken still belongs to its owner.
                let expected = model.held_by(worker, claim);
                let head = store.head().unwrap();
                let committed = match kind {
                    3 => {
                        let extended = queue::heartbeat(&store, claim, name, now, LEASE).unwrap();
                        if let Some(extended) = &extended {
                            model.lease_until = extended.lease_until;
                        }
                        let committed = extended.is_some();
                        if let Some(extended) = extended {
                            claims[chosen] = extended;
                        }
                        committed
                    }
                    4 => {
                        let done = queue::finish(&store, claim, name, "ok").unwrap();
                        // A repeated finish of a committed attempt is an exact
                        // retry, whatever happened to the job since.
                        if done && !expected {
                            assert_eq!(model.state, DONE);
                            assert_eq!(model.attempt, claim.attempt);
                            assert_eq!(store.head().unwrap(), head);
                            continue;
                        }
                        if done {
                            model.state = DONE;
                        }
                        done
                    }
                    5 => {
                        let run_after = now + step;
                        let released = queue::release(&store, claim, name, run_after).unwrap();
                        if released {
                            model.state = PENDING;
                            model.run_after = run_after;
                        }
                        released
                    }
                    _ => {
                        let failed = queue::fail(&store, claim, name, "no").unwrap();
                        if failed {
                            model.state = FAILED;
                        }
                        failed
                    }
                };
                assert_eq!(committed, expected, "kind {kind} by {name} on {model:?}");
                if !committed {
                    assert_eq!(
                        store.head().unwrap(),
                        head,
                        "a refused write commits nothing"
                    );
                }
            }
            _ => now += step,
        }
    }
    // The store agrees with the state machine, read back independently.
    let db = store.db().unwrap();
    let claimable: BTreeSet<EntityId> = queue::candidates(&db, QUEUE, now, 64)
        .unwrap()
        .into_iter()
        .map(|candidate| candidate.job)
        .collect();
    for model in jobs.values() {
        assert_eq!(state(&store, &model.entity), model.state);
        assert_eq!(
            one(&db, &model.entity, "job/attempt"),
            Some(Value::I64(model.attempt))
        );
        assert_eq!(
            one(&db, &model.entity, "job/owner"),
            model
                .owner
                .map(|owner| Value::String(WORKERS[owner].into()))
        );
        assert_eq!(claimable.contains(&model.entity), model.claimable(now));
        // Attempts only ever advance by one.
        let attempts: Vec<Value> = db
            .history(
                FactFilter {
                    entity: Some(model.entity.clone()),
                    attribute: Some(attr("job/attempt")),
                    value: None,
                },
                Revision::ZERO,
            )
            .unwrap()
            .into_iter()
            .filter(|event| event.operation == Operation::Assert)
            .map(|event| event.fact.value)
            .collect();
        assert_eq!(
            attempts,
            (0..=model.attempt).map(Value::I64).collect::<Vec<_>>()
        );
    }
    assert_eq!(
        claimable.len(),
        jobs.values().filter(|job| job.claimable(now)).count()
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]
    #[test]
    fn random_traces_match_the_claim_state_machine(
        operations in proptest::collection::vec((0_u8..8, 0_usize..3, 0_usize..4, 0_i64..150), 0..40)
    ) {
        run_trace(&operations);
    }
}

// Separate worker processes. Each test re-enters itself in child processes, as
// `process_recovery` does, so no helper binary exists and nothing here is
// counted twice. Children signal through files beside the database; no
// assertion depends on how long anything takes.

const STOP: &str = "stop";
const WAIT: Duration = Duration::from_secs(60);

fn wall_clock() -> i64 {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    i64::try_from(elapsed.as_millis()).unwrap()
}

struct KillOnDrop(Child);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn(test: &str, path: &Path, worker: usize, role: &str, lease_ms: i64) -> KillOnDrop {
    KillOnDrop(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--nocapture"])
            .env("FACTLITE_QUEUE_DB", path)
            .env("FACTLITE_QUEUE_WORKER", worker.to_string())
            .env("FACTLITE_QUEUE_ROLE", role)
            .env("FACTLITE_QUEUE_LEASE", lease_ms.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    )
}

/// Publishes a file under its final name only once it is complete.
fn publish(dir: &Path, name: &str, contents: &str) {
    let partial = dir.join(format!("{name}.partial"));
    std::fs::write(&partial, contents).unwrap();
    std::fs::rename(partial, dir.join(name)).unwrap();
}

fn await_file(dir: &Path, name: &str) -> String {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Ok(contents) = std::fs::read_to_string(dir.join(name)) {
            return contents;
        }
        assert!(Instant::now() < deadline, "{name} did not appear");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn await_exit(children: Vec<KillOnDrop>) {
    let deadline = Instant::now() + WAIT;
    for mut child in children {
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert!(status.success(), "a worker process failed: {status}");
                break;
            }
            assert!(Instant::now() < deadline, "a worker process did not exit");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Runs this process as a worker if the environment says it is one.
fn ran_as_worker() -> bool {
    let Some(path) = std::env::var_os("FACTLITE_QUEUE_DB") else {
        return false;
    };
    let variable = |name: &str| std::env::var(name).unwrap();
    let worker = variable("FACTLITE_QUEUE_WORKER");
    let lease_ms: i64 = variable("FACTLITE_QUEUE_LEASE").parse().unwrap();
    let name = format!("w{worker}");
    let dir = Path::new(&path).parent().unwrap().to_owned();
    let store = Store::open(&path).unwrap();
    publish(&dir, &format!("ready-{worker}"), "");
    if variable("FACTLITE_QUEUE_ROLE") == "stall" {
        // Claim one job and never finish it; the parent kills this process.
        loop {
            let claimed = queue::claim(&store, QUEUE, &name, wall_clock(), lease_ms, 1, 0).unwrap();
            if claimed.claim.is_some() {
                publish(&dir, "stalled", "");
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    let worked = queue::work_until_stopped(
        &store,
        QUEUE,
        STOP,
        &name,
        lease_ms,
        1,
        Duration::from_millis(1),
        wall_clock,
        |_, payload| payload.to_uppercase(),
    )
    .unwrap();
    publish(
        &dir,
        &format!("finished-{worker}"),
        &worked.finished.join("\n"),
    );
    true
}

/// Which worker reported finishing each job, failing on any job reported twice.
fn finished_by(dir: &Path, workers: impl Iterator<Item = usize>) -> BTreeMap<String, String> {
    let mut finished = BTreeMap::new();
    for worker in workers {
        for job in await_file(dir, &format!("finished-{worker}")).lines() {
            let earlier = finished.insert(job.to_owned(), format!("w{worker}"));
            assert_eq!(earlier, None, "{job} was finished twice");
        }
    }
    finished
}

fn asserted(db: &Db, job: &EntityId, name: &str) -> Vec<Value> {
    let filter = FactFilter {
        entity: Some(job.clone()),
        attribute: Some(attr(name)),
        value: None,
    };
    db.history(filter, Revision::ZERO)
        .unwrap()
        .into_iter()
        .filter(|event| event.operation == Operation::Assert)
        .map(|event| event.fact.value)
        .collect()
}

#[test]
fn worker_processes_finish_each_job_exactly_once() {
    if ran_as_worker() {
        return;
    }
    const TEST: &str = "worker_processes_finish_each_job_exactly_once";
    const WORKERS: usize = 4;
    const JOBS: usize = 100;
    let (dir, store) = open();
    let path = dir.path().join("queue.sqlite");
    // Leases far longer than the test, so no job is ever taken over.
    let children: Vec<_> = (0..WORKERS)
        .map(|worker| spawn(TEST, &path, worker, "work", 3_600_000))
        .collect();
    for worker in 0..WORKERS {
        await_file(dir.path(), &format!("ready-{worker}"));
    }
    // Every worker asks for the first claimable job, so each job is contended.
    let jobs: Vec<EntityId> = (0..JOBS)
        .map(|job| {
            queue::enqueue(&store, QUEUE, &format!("job-{job}"), &format!("p{job}"), 0).unwrap()
        })
        .collect();
    queue::enqueue(&store, "control", STOP, "", 0).unwrap();
    await_exit(children);

    let finished = finished_by(dir.path(), 0..WORKERS);
    assert_eq!(finished.len(), JOBS);
    let db = store.db().unwrap();
    for (index, job) in jobs.iter().enumerate() {
        let owner = &finished[&format!("job-{index}")];
        assert_eq!(state(&store, job), DONE);
        // Claimed once, by the worker that reported it, and by nobody else.
        assert_eq!(
            asserted(&db, job, "job/attempt"),
            [Value::I64(0), Value::I64(1)]
        );
        assert_eq!(
            asserted(&db, job, "job/owner"),
            [Value::String(owner.clone())]
        );
        assert_eq!(
            one(&db, job, "job/result"),
            Some(Value::String(format!("P{index}")))
        );
    }
    // The schema, each enqueue, the stop job, and one claim and one finish per
    // job. A lost claim commits nothing, so there is no other revision.
    assert_eq!(
        db.transactions(Revision::ZERO).unwrap().len(),
        1 + JOBS + 1 + 2 * JOBS
    );
}

#[test]
fn a_killed_workers_job_is_finished_by_another_process() {
    if ran_as_worker() {
        return;
    }
    const TEST: &str = "a_killed_workers_job_is_finished_by_another_process";
    const LEASE_MS: i64 = 300;
    let (dir, store) = open();
    let path = dir.path().join("queue.sqlite");
    let stalled = spawn(TEST, &path, 0, "stall", LEASE_MS);
    await_file(dir.path(), "ready-0");
    let abandoned = queue::enqueue(&store, QUEUE, "abandoned", "left", 0).unwrap();
    await_file(dir.path(), "stalled");
    drop(stalled);
    assert_eq!(state(&store, &abandoned), CLAIMED);

    let children: Vec<_> = (1..3)
        .map(|worker| spawn(TEST, &path, worker, "work", LEASE_MS))
        .collect();
    let mut jobs = vec![("abandoned".to_owned(), abandoned.clone())];
    for job in 0..10 {
        let id = format!("job-{job}");
        let entity = queue::enqueue(&store, QUEUE, &id, "p", 0).unwrap();
        jobs.push((id, entity));
    }
    queue::enqueue(&store, "control", STOP, "", 0).unwrap();
    await_exit(children);

    let finished = finished_by(dir.path(), 1..3);
    assert_eq!(finished.len(), jobs.len());
    let db = store.db().unwrap();
    for (id, job) in &jobs {
        // Done exactly once, by the worker that reported it. A slow machine
        // may let a live lease pass, so other jobs may also show two attempts.
        let done = Value::Keyword(factlite::Keyword::new(DONE).unwrap());
        let states = asserted(&db, job, "job/state");
        assert_eq!(states.iter().filter(|state| **state == done).count(), 1);
        assert_eq!(states.last(), Some(&done));
        assert_eq!(
            asserted(&db, job, "job/owner").last(),
            Some(&Value::String(finished[id].clone()))
        );
    }
    // The killed worker's claim is in history, and its job was taken over.
    let owners = asserted(&db, &abandoned, "job/owner");
    assert_eq!(owners[0], Value::String("w0".into()));
    assert!(owners.len() >= 2 && owners[1..].iter().all(|owner| *owner != owners[0]));
    assert_eq!(
        one(&db, &abandoned, "job/result"),
        Some(Value::String("LEFT".into()))
    );
}
