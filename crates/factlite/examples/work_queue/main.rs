//! A durable work queue on Factlite. See `docs/queues.md`.
//!
//! With no arguments this walks through one queue in one process and checks
//! each step. With `--workers N --jobs M` it starts N worker processes on one
//! file, has them drain M jobs, and prints one JSON record of what it measured:
//!
//! ```sh
//! cargo run --release -p factlite --example work_queue
//! cargo run --release -p factlite --example work_queue -- --workers 4 --jobs 2000
//! ```
//!
//! Options for the measured run: `--spread K` (candidates each worker chooses
//! among, default 1) and `--directory PATH` (default: the OS temporary
//! directory). The database is deleted afterwards.

#[allow(dead_code, reason = "the walkthrough does not call every operation")]
mod queue;

use factlite::{FactFilter, Operation, Revision, Store, Value};
use queue::attr;
use std::{
    error::Error,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const QUEUE: &str = "work";
const STOP: &str = "stop";
/// Longer than any measured run, so no lease passes.
const LEASE_MS: i64 = 3_600_000;

fn wall_clock() -> i64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after 1970");
    i64::try_from(elapsed.as_millis()).expect("the clock is before the year 292278994")
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut workers = None;
    let mut jobs = None;
    let mut spread = 1_u64;
    let mut directory = None;
    let mut worker = None;
    let mut database = None;
    let mut start_at = 0;
    let mut args = std::env::args().skip(1);
    while let Some(option) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("{option} needs a value"))?;
        match option.as_str() {
            "--workers" => workers = Some(value.parse::<usize>()?),
            "--jobs" => jobs = Some(value.parse::<usize>()?),
            "--spread" => spread = value.parse()?,
            "--directory" => directory = Some(PathBuf::from(value)),
            // Used by the measured run to start its own workers.
            "--worker" => worker = Some(value),
            "--database" => database = Some(PathBuf::from(value)),
            "--start-at" => start_at = value.parse()?,
            _ => return Err(format!("unknown option {option}").into()),
        }
    }
    if spread == 0 {
        return Err("--spread must be positive".into());
    }
    match (worker, database, workers, jobs) {
        (Some(worker), Some(database), None, None) => work(&worker, &database, spread, start_at),
        (None, None, Some(workers), Some(jobs)) if workers > 0 && jobs > 0 => {
            measure(workers, jobs, spread, directory.as_deref())
        }
        (None, None, None, None) => walk_through(),
        _ => Err("give no options, or positive --workers and --jobs".into()),
    }
}

/// One queue, one process, a caller-supplied clock.
fn walk_through() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path().join("queue.sqlite"))?;
    queue::define_schema(&store)?;
    let lease = 1_000;

    // Enqueue is exact on retry: the second call returns the same job.
    let report = queue::enqueue(&store, QUEUE, "report-7", "summarize the run", 0)?;
    assert_eq!(
        queue::enqueue(&store, QUEUE, "report-7", "summarize the run", 0)?,
        report
    );
    let later = queue::enqueue(&store, QUEUE, "cleanup-7", "remove scratch files", 5_000)?;

    // Two workers read the same candidate. One claim commits.
    let seen = queue::candidates(&store.db()?, QUEUE, 0, 1)?;
    let ada = queue::try_claim(&store, &seen[0], "ada", 0, lease)?.expect("ada is first");
    assert_eq!(queue::try_claim(&store, &seen[0], "grace", 0, lease)?, None);
    assert_eq!((ada.job.clone(), ada.attempt), (report.clone(), 1));
    // The other job is not due, so there is nothing else to claim.
    assert_eq!(
        queue::claim(&store, QUEUE, "grace", 0, lease, 1, 0)?.claim,
        None
    );

    // Ada stops responding. After the lease, Grace takes the job over.
    let grace = queue::claim(&store, QUEUE, "grace", lease + 1, lease, 1, 0)?
        .claim
        .expect("the lease has passed");
    assert_eq!((grace.job.clone(), grace.attempt), (report.clone(), 2));
    // Ada comes back. Her claim is from attempt 1 and can no longer commit.
    assert!(!queue::finish(&store, &ada, "ada", "late answer")?);
    assert!(queue::finish(
        &store,
        &grace,
        "grace",
        "three tests failed"
    )?);

    // The delayed job becomes claimable at its time and is released for later.
    let cleanup = queue::claim(&store, QUEUE, "ada", 5_000, lease, 1, 0)?
        .claim
        .expect("the cleanup job is due");
    assert_eq!(cleanup.job, later);
    assert!(queue::release(&store, &cleanup, "ada", 60_000)?);
    assert!(queue::unfinished(&store.db()?, QUEUE)?);

    // Every state the first job passed through is still there.
    for name in ["job/state", "job/owner", "job/result"] {
        let filter = FactFilter {
            entity: Some(report.clone()),
            attribute: Some(attr(name)),
            value: None,
        };
        let values: Vec<String> = store
            .db()?
            .history(filter, Revision::ZERO)?
            .into_iter()
            .filter(|event| event.operation == Operation::Assert)
            .map(|event| match event.fact.value {
                Value::Keyword(keyword) => keyword.as_str().to_owned(),
                Value::String(text) => text,
                other => unreachable!("{name} holds keywords or strings, not {other:?}"),
            })
            .collect();
        println!("{name}: {}", values.join(" -> "));
    }
    Ok(())
}

/// Median time of one `candidates` call, the read every claim starts with.
fn candidate_read_ms(store: &Store, ahead_ms: i64) -> Result<f64, Box<dyn Error>> {
    let db = store.db()?;
    let now = wall_clock() + ahead_ms;
    let mut samples = Vec::new();
    for _ in 0..21 {
        let reading = Instant::now();
        queue::candidates(&db, QUEUE, now, 1)?;
        samples.push(reading.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    Ok(samples[samples.len() / 2])
}

/// A worker process of the measured run.
fn work(worker: &str, database: &Path, spread: u64, start_at: i64) -> Result<(), Box<dyn Error>> {
    let store = Store::open(database)?;
    // Every worker opens first and starts claiming at the same instant.
    while wall_clock() < start_at {
        std::thread::sleep(Duration::from_millis(1));
    }
    let worked = queue::work_until_stopped(
        &store,
        QUEUE,
        STOP,
        worker,
        LEASE_MS,
        spread,
        Duration::from_millis(1),
        wall_clock,
        |_, payload| payload.to_owned(),
    )?;
    println!("{} {}", worked.finished.len(), worked.lost);
    Ok(())
}

/// Starts `workers` processes on one file and times how long they take to
/// drain `jobs` jobs, each due one millisecond after the one before.
fn measure(
    workers: usize,
    jobs: usize,
    spread: u64,
    directory: Option<&Path>,
) -> Result<(), Box<dyn Error>> {
    let directory = match directory {
        Some(path) => tempfile::tempdir_in(path)?,
        None => tempfile::tempdir()?,
    };
    let database = directory.path().join("queue.sqlite");
    let store = Store::open(&database)?;
    queue::define_schema(&store)?;
    // All due already, at distinct times, as jobs enqueued over time would be.
    let due = wall_clock() - i64::try_from(jobs)?;
    for job in 0..jobs {
        let run_after = due + i64::try_from(job)?;
        queue::enqueue(&store, QUEUE, &format!("job-{job}"), "payload", run_after)?;
    }
    queue::enqueue(&store, "control", STOP, "", 0)?;
    // One candidate read while every job is pending and none is finished.
    let read_at_start_ms = candidate_read_ms(&store, 10_000)?;
    drop(store);

    // Two seconds from now, by which time every worker has opened the store.
    let start = wall_clock() + 2_000;
    let children: Vec<_> = (0..workers)
        .map(|worker| {
            Command::new(std::env::current_exe()?)
                .args(["--worker", &format!("w{worker}")])
                .args(["--database".as_ref(), database.as_os_str()])
                .args(["--spread", &spread.to_string()])
                .args(["--start-at", &start.to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .spawn()
        })
        .collect::<Result<_, _>>()?;
    let mut finished = 0;
    let mut lost = 0;
    for child in children {
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(format!("a worker failed: {}", output.status).into());
        }
        let text = String::from_utf8(output.stdout)?;
        let mut counts = text.split_whitespace().map(str::parse::<u64>);
        finished += counts.next().ok_or("missing finished count")??;
        lost += counts.next().ok_or("missing lost count")??;
    }
    let elapsed_ms = wall_clock() - start;
    if finished != jobs as u64 {
        return Err(format!("{finished} of {jobs} jobs were finished").into());
    }

    let opening = Instant::now();
    let store = Store::open(&database)?;
    let open_ms = opening.elapsed().as_secs_f64() * 1000.0;
    let revisions = store.db()?.transactions(Revision::ZERO)?.len();
    // The same read once every job is finished and none is pending.
    let read_at_end_ms = candidate_read_ms(&store, 10_000)?;
    // And once more at a time by which every lease ever granted has passed.
    let read_after_leases_ms = candidate_read_ms(&store, 2 * LEASE_MS)?;
    // Checkpoint through SQLite's own close so the size is the main file's.
    drop(store);
    let bytes = std::fs::metadata(&database)?.len();
    println!(
        "{{\"workers\":{workers},\"jobs\":{jobs},\"spread\":{spread},\
         \"drain_ms\":{elapsed_ms},\"jobs_per_second\":{:.0},\"lost_claims\":{lost},\
         \"candidate_read_ms_at_start\":{read_at_start_ms:.3},\
         \"candidate_read_ms_at_end\":{read_at_end_ms:.3},\
         \"candidate_read_ms_after_leases\":{read_after_leases_ms:.3},\
         \"revisions\":{revisions},\"file_bytes\":{bytes},\"writable_open_ms\":{open_ms:.1}}}",
        jobs as f64 * 1000.0 / elapsed_ms as f64,
    );
    Ok(())
}
