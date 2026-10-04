//! Reproducible local-file workloads with independent answer checks, not an SLA.
use factlite::{
    AttributeDefinition, Cardinality, Db, Direction, FactFilter, Order, Params, Pattern, Query,
    QueryName, Store, TempId, Term, Tx, Uniqueness, Value, ValueTerm, ValueType,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    error::Error,
    time::{Duration, Instant},
};

struct Config {
    tasks: usize,
    revisions: usize,
    samples: usize,
    directory: std::path::PathBuf,
}
fn configurations() -> Result<Vec<Config>, Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let mut options = BTreeMap::new();
    let mut directory = None;
    while let Some(option) = args.next() {
        if option == "--directory" {
            let path = args.next().ok_or("missing directory path")?;
            if directory.replace(std::path::PathBuf::from(path)).is_some() {
                return Err("duplicate directory".into());
            }
            continue;
        }
        if !["--tasks", "--revisions", "--samples"].contains(&option.as_str()) {
            return Err(format!(
                "unknown option: {option}; use --tasks N --revisions N --samples N"
            )
            .into());
        }
        let value: usize = args.next().ok_or("missing option value")?.parse()?;
        if value == 0 || options.insert(option, value).is_some() {
            return Err("counts must be positive; duplicate options fail".into());
        }
    }
    let directory = directory.unwrap_or_else(std::env::temp_dir);
    if options.is_empty() {
        return Ok([(10, 10), (100, 50), (1000, 50), (100, 250)]
            .into_iter()
            .map(|(tasks, revisions)| Config {
                tasks,
                revisions,
                samples: 15,
                directory: directory.clone(),
            })
            .collect());
    }
    Ok(vec![Config {
        tasks: *options.get("--tasks").ok_or("--tasks required")?,
        revisions: *options.get("--revisions").ok_or("--revisions required")?,
        samples: options.get("--samples").copied().unwrap_or(15),
        directory,
    }])
}
fn timed<T>(operation: impl FnOnce() -> factlite::Result<T>) -> factlite::Result<(T, Duration)> {
    let start = Instant::now();
    let result = operation()?;
    Ok((result, start.elapsed()))
}
fn summary(durations: Vec<Duration>) -> serde_json::Value {
    let mut milliseconds: Vec<_> = durations
        .iter()
        .map(Duration::as_secs_f64)
        .map(|s| s * 1000.0)
        .collect();
    milliseconds.sort_by(f64::total_cmp);
    let percentile95 = (milliseconds.len() * 95).div_ceil(100) - 1;
    json!({"samples":milliseconds.len(),"median_ms":milliseconds[milliseconds.len()/2],"p95_ms":milliseconds[percentile95]})
}
fn name(text: &str) -> QueryName {
    QueryName::new(text).expect("fixed name")
}
fn var(text: &str) -> Term {
    Term::Var(name(text))
}
fn pattern(entity: &str, attr: &str, value: Term) -> Pattern {
    Pattern {
        entity: var(entity),
        attribute: attr.parse().expect("fixed qualified attribute"),
        value,
    }
}
fn definition(attr: &str, ty: ValueType) -> AttributeDefinition {
    AttributeDefinition::new(
        attr.parse().expect("fixed attribute"),
        ty,
        Cardinality::One,
        Uniqueness::None,
    )
    .expect("fixed definition")
}
fn status(index: usize) -> &'static str {
    if index % 2 == 1 { "doing" } else { "todo" }
}
fn workload(config: Config) -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir_in(&config.directory)?;
    let path = dir.path().join("workload.sqlite");
    let store = Store::open(&path)?;
    let person = TempId::new("person")?;
    let mut seed = Tx::builder()
        .idempotency_key("seed")
        .define_attribute(definition("person/name", ValueType::String))
        .define_attribute(definition("task/title", ValueType::String))
        .define_attribute(definition("task/status", ValueType::String))
        .define_attribute(definition("task/owner", ValueType::Ref))
        .new_entity(person.clone())
        .assert(
            person.clone(),
            "person/name".parse()?,
            Value::String("Ada".into()),
        );
    let mut labels = Vec::new();
    for index in 0..config.tasks {
        let label = TempId::new(format!("task{index}"))?;
        seed = seed
            .new_entity(label.clone())
            .assert(
                label.clone(),
                "task/title".parse()?,
                Value::String(format!("Task {index:08}")),
            )
            .assert(
                label.clone(),
                "task/status".parse()?,
                Value::String(status(index).into()),
            )
            .assert(
                label.clone(),
                "task/owner".parse()?,
                ValueTerm::TempRef(person.clone()),
            );
        labels.push(label);
    }
    let seed = seed.build()?;
    let (a, seed_time) = timed(|| store.commit(seed))?;
    let ids: Vec<_> = labels
        .iter()
        .map(|label| a.tempids[label].clone())
        .collect();
    let mut first_status = "todo";
    for revision in 1..config.revisions {
        first_status = status(revision);
        store.commit(
            Tx::builder()
                .assert(
                    ids[0].clone(),
                    "task/status".parse()?,
                    Value::String(first_status.into()),
                )
                .metadata("step", Value::I64(i64::try_from(revision)?))
                .idempotency_key(format!("step{revision}"))
                .build()?,
        )?;
    }
    let head = store.head()?;
    let current = store.db()?;
    let old = store.db_at(&a.after)?;
    let query = Query {
        inputs: [(name("wanted"), ValueType::String)].into(),
        patterns: vec![
            pattern("task", "task/status", Term::Param(name("wanted"))),
            pattern("task", "task/title", var("title")),
            pattern("task", "task/owner", var("person")),
            pattern("person", "person/name", var("name")),
        ],
        select: vec![name("task"), name("title"), name("name")],
        order: vec![
            Order {
                field: name("title"),
                direction: Direction::Asc,
            },
            Order {
                field: name("task"),
                direction: Direction::Asc,
            },
        ],
        limit: Some(20),
        ..Query::default()
    };
    let params: Params = [(name("wanted"), Value::String("doing".into()))].into();
    let expected = |first_status: &str| {
        (0..config.tasks)
            .filter(|index| {
                if *index == 0 {
                    first_status == "doing"
                } else {
                    status(*index) == "doing"
                }
            })
            .take(20)
            .map(|index| {
                vec![
                    Value::Ref(ids[index].clone()),
                    Value::String(format!("Task {index:08}")),
                    Value::String("Ada".into()),
                ]
            })
            .collect::<Vec<_>>()
    };
    let expected_old = expected("todo");
    let expected_head = expected(first_status);
    let mut heads = Vec::new();
    let mut opens = Vec::new();
    let mut historical = Vec::new();
    let mut at_head = Vec::new();
    let mut facts = Vec::new();
    let mut commits = Vec::new();
    // Warm paths once. Answer assertions and setup stay outside timed operations.
    assert_eq!(old.query(&query, &params)?.rows, expected_old);
    assert_eq!(current.query(&query, &params)?.rows, expected_head);
    for _ in 0..config.samples {
        let (actual, elapsed) = timed(|| store.head())?;
        assert_eq!(actual, head);
        heads.push(elapsed);
        let (reader, elapsed) = timed(|| Db::open_read_only(&path))?;
        assert_eq!(*reader.snapshot(), head);
        opens.push(elapsed);
        drop(reader);
        let (actual, elapsed) = timed(|| old.query(&query, &params))?;
        assert_eq!(actual.rows, expected_old);
        historical.push(elapsed);
        let (actual, elapsed) = timed(|| current.query(&query, &params))?;
        assert_eq!(actual.rows, expected_head);
        at_head.push(elapsed);
        let (actual, elapsed) = timed(|| current.facts(FactFilter::default()))?;
        assert_eq!(actual.len(), config.tasks * 3 + 1);
        facts.push(elapsed);
    }
    for index in 0..config.samples {
        let request = Tx::builder()
            .metadata("noop", Value::I64(i64::try_from(index)?))
            .idempotency_key(format!("noop{index}"))
            .build()?;
        let (report, elapsed) = timed(|| store.commit(request))?;
        assert!(report.effective_delta.is_empty());
        commits.push(elapsed);
    }
    assert_eq!(current.query(&query, &params)?.rows, expected_head);
    assert_eq!(old.query(&query, &params)?.rows, expected_old);
    assert_eq!(
        store.head()?.revision().get(),
        u64::try_from(config.revisions + config.samples)?
    );
    println!(
        "{}",
        json!({"version":1,"workload":"task-owner-join","directory":config.directory,"sqlite":rusqlite::version(),"tasks":config.tasks,"entities":config.tasks+1,"facts":config.tasks*3+1,"basis_revision":config.revisions,"historical_revision":1,"samples_per_operation":config.samples,"seed_commit_ms":seed_time.as_secs_f64()*1000.0,"head":summary(heads),"read_only_open":summary(opens),"historical_query":summary(historical),"pinned_head_query":summary(at_head),"facts_at_head":summary(facts),"new_noop_commit":summary(commits)})
    );
    Ok(())
}
fn main() -> Result<(), Box<dyn Error>> {
    for config in configurations()? {
        workload(config)?;
    }
    Ok(())
}
