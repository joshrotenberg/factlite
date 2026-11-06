//! `not` and `not-join`: expected results at historical views, lexical scope
//! rules, and errors that come before any data is read.

use factlite::{
    Aggregate, AggregateFn, AttributeDefinition, Cardinality, Comparison, Db, Direction, EntityId,
    ErrorCode, Negation, Order, Params, Pattern, Predicate, Query, QueryName, Store, TempId, Term,
    Tx, Uniqueness, Value, ValueType, wire,
};

fn name(value: &str) -> QueryName {
    QueryName::new(value).unwrap()
}
fn var(value: &str) -> Term {
    Term::Var(name(value))
}
fn string(value: &str) -> Term {
    Term::Const(Value::String(value.into()))
}
fn pattern(entity: Term, attribute: &str, value: Term) -> Pattern {
    Pattern {
        entity,
        attribute: attribute.parse().unwrap(),
        value,
    }
}
fn not(patterns: Vec<Pattern>, predicates: Vec<Predicate>) -> Negation {
    Negation {
        join: None,
        patterns,
        predicates,
    }
}
fn not_join(join: &[&str], patterns: Vec<Pattern>, predicates: Vec<Predicate>) -> Negation {
    Negation {
        join: Some(join.iter().map(|n| name(n)).collect()),
        patterns,
        predicates,
    }
}
fn definition(attribute: &str, ty: ValueType) -> AttributeDefinition {
    AttributeDefinition::new(
        attribute.parse().unwrap(),
        ty,
        Cardinality::Many,
        Uniqueness::None,
    )
    .unwrap()
}

/// Runs by status, each listed in `runs` by label.
fn runs_query(negations: Vec<Negation>) -> Query {
    Query {
        patterns: vec![pattern(var("r"), "run/status", var("s"))],
        negations,
        select: vec![name("r")],
        ..Query::default()
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    store: Store,
    /// run1 done/ok, run2 done/failed, run3 running with no outcome.
    at_a: Db,
    runs: Vec<EntityId>,
}

/// At A: run1 done "ok", run2 done "failed", run3 running with no outcome.
/// After A: run3 gains "ok", run1 loses its outcome, and run4 starts running.
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("runs.sqlite")).unwrap();
    let label = |n: usize| TempId::new(format!("run{n}")).unwrap();
    let mut tx = Tx::builder()
        .define_attribute(definition("run/status", ValueType::String))
        .define_attribute(definition("run/outcome", ValueType::String))
        .define_attribute(definition("run/cost", ValueType::I64));
    for (n, status, outcome) in [
        (1, "done", Some("ok")),
        (2, "done", Some("failed")),
        (3, "running", None),
        (4, "pending", None),
    ] {
        tx = tx.new_entity(label(n)).assert(
            label(n),
            "run/cost".parse().unwrap(),
            Value::I64(n as i64),
        );
        if n < 4 {
            tx = tx.assert(
                label(n),
                "run/status".parse().unwrap(),
                Value::String(status.into()),
            );
        }
        if let Some(outcome) = outcome {
            tx = tx.assert(
                label(n),
                "run/outcome".parse().unwrap(),
                Value::String(outcome.into()),
            );
        }
    }
    let report = store.commit(tx.build().unwrap()).unwrap();
    let runs = report.created_entities.clone();
    let at_a = store.db().unwrap();
    store
        .commit(
            Tx::builder()
                .assert(
                    runs[2].clone(),
                    "run/outcome".parse().unwrap(),
                    Value::String("ok".into()),
                )
                .retract(
                    runs[0].clone(),
                    "run/outcome".parse().unwrap(),
                    Value::String("ok".into()),
                )
                .assert(
                    runs[3].clone(),
                    "run/status".parse().unwrap(),
                    Value::String("running".into()),
                )
                .build()
                .unwrap(),
        )
        .unwrap();
    Fixture {
        dir,
        store,
        at_a,
        runs,
    }
}

fn entities(db: &Db, query: &Query, params: &Params) -> Vec<EntityId> {
    let mut found: Vec<_> = db
        .query(query, params)
        .unwrap()
        .rows
        .into_iter()
        .map(|row| match &row[..] {
            [Value::Ref(id)] => id.clone(),
            other => panic!("expected one ref column: {other:?}"),
        })
        .collect();
    found.sort();
    found
}
fn sorted(mut ids: Vec<EntityId>) -> Vec<EntityId> {
    ids.sort();
    ids
}

#[test]
fn absence_is_evaluated_at_the_view_revision() {
    let f = fixture();
    let absent = runs_query(vec![not_join(
        &["r"],
        vec![pattern(var("r"), "run/outcome", var("o"))],
        vec![],
    )]);
    let none = Params::new();
    // At A only run3 lacks an outcome. Later, run3 gains one, run1 loses its
    // outcome, and run4 gets a status: none of that is visible at A.
    assert_eq!(entities(&f.at_a, &absent, &none), vec![f.runs[2].clone()]);
    assert_eq!(
        entities(&f.store.db().unwrap(), &absent, &none),
        sorted(vec![f.runs[0].clone(), f.runs[3].clone()])
    );
    // A reopened store pinned at A agrees.
    let a = f.at_a.snapshot().clone();
    let Fixture {
        dir,
        store,
        at_a,
        runs,
    } = f;
    drop((store, at_a));
    let reopened = Store::open(dir.path().join("runs.sqlite")).unwrap();
    assert_eq!(
        entities(&reopened.db_at(&a).unwrap(), &absent, &none),
        vec![runs[2].clone()]
    );
}

#[test]
fn not_excludes_bound_matches_and_not_join_filters_locals() {
    let f = fixture();
    let none = Params::new();
    // `not` with a constant: runs whose outcome is not "failed", including
    // runs with no outcome at all.
    let not_failed = runs_query(vec![not(
        vec![pattern(var("r"), "run/outcome", string("failed"))],
        vec![],
    )]);
    assert_eq!(
        entities(&f.at_a, &not_failed, &none),
        sorted(vec![f.runs[0].clone(), f.runs[2].clone()])
    );
    // `not-join` with a predicate on a local and a parameter: no outcome
    // other than the wanted one.
    let only_wanted = Query {
        inputs: [(name("wanted"), ValueType::String)].into(),
        ..runs_query(vec![not_join(
            &["r"],
            vec![pattern(var("r"), "run/outcome", var("o"))],
            vec![Predicate {
                left: var("o"),
                comparison: Comparison::Ne,
                right: Term::Param(name("wanted")),
            }],
        )])
    };
    let wanted: Params = [(name("wanted"), Value::String("ok".into()))].into();
    assert_eq!(
        entities(&f.at_a, &only_wanted, &wanted),
        sorted(vec![f.runs[0].clone(), f.runs[2].clone()])
    );
    // Two groups are conjunctive: neither may hold.
    let neither = runs_query(vec![
        not(
            vec![],
            vec![Predicate {
                left: var("s"),
                comparison: Comparison::Eq,
                right: string("running"),
            }],
        ),
        not(vec![pattern(var("r"), "run/outcome", string("ok"))], vec![]),
    ]);
    assert_eq!(entities(&f.at_a, &neither, &none), vec![f.runs[1].clone()]);
    // A group whose typed equality cannot hold never excludes anything.
    let mixed = runs_query(vec![not(
        vec![],
        vec![Predicate {
            left: string(""),
            comparison: Comparison::Eq,
            right: Term::Const(Value::Bytes(Vec::new())),
        }],
    )]);
    assert_eq!(entities(&f.at_a, &mixed, &none).len(), 3);
}

#[test]
fn uncorrelated_groups_and_empty_joins() {
    let f = fixture();
    let none = Params::new();
    // No positive patterns: one empty row when nothing matches the group.
    let no_failure = |outcome: &str| Query {
        negations: vec![not_join(
            &[],
            vec![pattern(var("x"), "run/outcome", string(outcome))],
            vec![],
        )],
        ..Query::default()
    };
    assert_eq!(
        f.at_a.query(&no_failure("cancelled"), &none).unwrap().rows,
        vec![Vec::<Value>::new()]
    );
    assert!(
        f.at_a
            .query(&no_failure("failed"), &none)
            .unwrap()
            .rows
            .is_empty()
    );
    // An unused join variable and an empty join list both correlate on
    // nothing: any run with an outcome removes every row.
    let unused = runs_query(vec![not_join(
        &["s"],
        vec![pattern(var("x"), "run/outcome", var("o"))],
        vec![],
    )]);
    assert!(entities(&f.at_a, &unused, &none).is_empty());
}

#[test]
fn negation_filters_tuples_before_aggregation() {
    let f = fixture();
    // Total cost per status of runs with no outcome. run4 has no status at A.
    let q = Query {
        patterns: vec![
            pattern(var("r"), "run/status", var("s")),
            pattern(var("r"), "run/cost", var("c")),
        ],
        negations: vec![not_join(
            &["r"],
            vec![pattern(var("r"), "run/outcome", var("o"))],
            vec![],
        )],
        select: vec![name("s")],
        with: vec![name("r")],
        aggregates: vec![
            Aggregate {
                function: AggregateFn::Count,
                var: name("r"),
                name: name("n"),
            },
            Aggregate {
                function: AggregateFn::Sum,
                var: name("c"),
                name: name("total"),
            },
        ],
        order: vec![Order {
            field: name("s"),
            direction: Direction::Asc,
        }],
        ..Query::default()
    };
    let none = Params::new();
    assert_eq!(
        f.at_a.query(&q, &none).unwrap().rows,
        vec![vec![
            Value::String("running".into()),
            Value::I64(1),
            Value::I64(3)
        ]]
    );
    // At head run1 (done, cost 1) and run4 (running, cost 4) lack outcomes.
    assert_eq!(
        f.store.db().unwrap().query(&q, &none).unwrap().rows,
        vec![
            vec![Value::String("done".into()), Value::I64(1), Value::I64(1)],
            vec![
                Value::String("running".into()),
                Value::I64(1),
                Value::I64(4)
            ],
        ]
    );
    // Eliminating every tuple leaves no groups.
    let all_gone = Query {
        negations: vec![not(
            vec![],
            vec![Predicate {
                left: var("c"),
                comparison: Comparison::Ge,
                right: Term::Const(Value::I64(0)),
            }],
        )],
        ..q
    };
    assert!(f.at_a.query(&all_gone, &none).unwrap().rows.is_empty());
}

#[test]
fn groups_beyond_the_sql_budget_give_the_same_results() {
    let f = fixture();
    let small = runs_query(vec![not_join(
        &["r"],
        vec![pattern(var("r"), "run/outcome", var("o"))],
        vec![],
    )]);
    // 65 group aliases plus the outer one exceed SQLite's 64-table join
    // limit, so this query succeeds only because group patterns count toward
    // the budget and send it to the reference evaluator.
    let large = runs_query(vec![not_join(
        &["r"],
        vec![pattern(var("r"), "run/outcome", var("o")); 65],
        vec![],
    )]);
    let none = Params::new();
    assert_eq!(
        entities(&f.at_a, &large, &none),
        entities(&f.at_a, &small, &none)
    );
    assert_eq!(entities(&f.at_a, &large, &none), vec![f.runs[2].clone()]);
}

fn code(db: &Db, query: &Query) -> ErrorCode {
    let error = db.validate_query(query, &Params::new()).unwrap_err();
    // Execution reports the same error, before reading data, even with limit 0.
    let limited = Query {
        limit: Some(0),
        ..query.clone()
    };
    assert_eq!(
        db.query(&limited, &Params::new()).unwrap_err().code(),
        error.code()
    );
    error.code()
}

#[test]
fn scope_rules_are_enforced() {
    let f = fixture();
    let db = &f.at_a;
    let outcome = |o: &str| pattern(var("r"), "run/outcome", var(o));
    let ne = |left: &str, right: Term| Predicate {
        left: var(left),
        comparison: Comparison::Ne,
        right,
    };
    let invalid = [
        // `not` may not introduce variables, in patterns or predicates.
        runs_query(vec![not(vec![outcome("o")], vec![])]),
        runs_query(vec![not(vec![], vec![ne("o", string("x"))])]),
        // Join variables must be bound outside, and listed once.
        runs_query(vec![not_join(&["q"], vec![outcome("o")], vec![])]),
        runs_query(vec![not_join(&["r", "r"], vec![outcome("o")], vec![])]),
        // A local may not shadow an outer variable.
        runs_query(vec![not_join(&["r"], vec![outcome("s")], vec![])]),
        // A local must be bound by a group pattern, not only a predicate.
        runs_query(vec![not_join(&["r"], vec![], vec![ne("o", string("x"))])]),
        // An outer variable not in the join list is not visible.
        runs_query(vec![not_join(
            &["r"],
            vec![outcome("o")],
            vec![ne("s", string("x"))],
        )]),
        // Locals do not escape into selection, `with` or aggregates.
        Query {
            select: vec![name("o")],
            ..runs_query(vec![not_join(&["r"], vec![outcome("o")], vec![])])
        },
        // An empty group.
        runs_query(vec![not(vec![], vec![])]),
    ];
    for query in &invalid {
        assert_eq!(code(db, query), ErrorCode::InvalidRequest, "{query:?}");
    }
    // Types inside a group are checked against outer and local types.
    assert_eq!(
        code(
            db,
            &runs_query(vec![not(
                vec![pattern(var("r"), "run/status", Term::Const(Value::I64(1)))],
                vec![]
            )])
        ),
        ErrorCode::TypeMismatch
    );
    assert_eq!(
        code(
            db,
            &runs_query(vec![not_join(
                &["s"],
                vec![pattern(var("s"), "run/status", var("o"))],
                vec![]
            )])
        ),
        ErrorCode::TypeMismatch
    );
    // Sibling groups have separate scopes: the same local name may have
    // different types in each.
    let siblings = runs_query(vec![
        not_join(&["r"], vec![outcome("o")], vec![]),
        not_join(
            &["r"],
            vec![pattern(var("r"), "run/cost", var("o"))],
            vec![],
        ),
    ]);
    assert_eq!(
        entities(db, &siblings, &Params::new()),
        Vec::<EntityId>::new()
    );
}

#[test]
fn unknown_group_attributes_fail_at_the_view_revision() {
    let f = fixture();
    let later = "run/owner";
    let group = |attribute: &str| {
        runs_query(vec![not_join(
            &["r"],
            vec![pattern(var("r"), attribute, var("o"))],
            vec![],
        )])
    };
    // Unknown, not trivially absent, even though the positive pattern matches.
    assert_eq!(code(&f.at_a, &group(later)), ErrorCode::UnknownAttribute);
    // Also when the positive patterns match nothing.
    let nothing = Query {
        predicates: vec![Predicate {
            left: var("s"),
            comparison: Comparison::Eq,
            right: string("never"),
        }],
        ..group(later)
    };
    assert_eq!(code(&f.at_a, &nothing), ErrorCode::UnknownAttribute);
    // Defined after A: still unknown at A, known at head.
    f.store
        .commit(
            Tx::builder()
                .define_attribute(definition(later, ValueType::String))
                .build()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(code(&f.at_a, &group(later)), ErrorCode::UnknownAttribute);
    assert_eq!(
        entities(&f.store.db().unwrap(), &group(later), &Params::new()).len(),
        4
    );
}

#[test]
fn wire_groups_distinguish_not_from_an_empty_join() {
    let envelope = |group: &str| {
        format!(
            r#"{{"version":1,"query":{{"match":[[{{"var":"r"}},"run/status",{{"var":"s"}}]],"select":["r"],"not":[{group}]}}}}"#
        )
    };
    let (q, _) = wire::decode_query(&envelope(
        r#"{"match":[[{"var":"r"},"run/outcome",{"string":"failed"}]]}"#,
    ))
    .unwrap();
    assert_eq!(q.negations[0].join, None);
    let (q, _) = wire::decode_query(&envelope(
        r#"{"join":[],"match":[[{"var":"x"},"run/outcome",{"string":"failed"}]]}"#,
    ))
    .unwrap();
    assert_eq!(q.negations[0].join, Some(Vec::new()));
    let (q, params) = wire::decode_query(&envelope(
        r#"{"join":["r"],"match":[[{"var":"r"},"run/outcome",{"var":"o"}]],"where":[{"left":{"var":"o"},"op":"ne","right":{"string":"ok"}}]}"#,
    ))
    .unwrap();
    assert_eq!(
        wire::decode_query(&wire::encode_query(&q, &params)).unwrap(),
        (q, params)
    );
    for bad in [
        "null",
        "[]",
        r#"{"join":null,"match":[]}"#,
        r#"{"join":"r","match":[]}"#,
        r#"{"match":[],"not":[]}"#,
        r#"{"match":[],"extra":1}"#,
        r#"{"match":[],"match":[]}"#,
        r#"{"match":{}}"#,
    ] {
        assert_eq!(
            wire::decode_query(&envelope(bad)).unwrap_err().code(),
            ErrorCode::InvalidRequest,
            "{bad}"
        );
    }
    // Queries without groups encode without a `not` member.
    let (q, params) =
        wire::decode_query(r#"{"version":1,"query":{"match":[],"select":[]}}"#).unwrap();
    assert!(!wire::encode_query(&q, &params).contains("\"not\""));
}
