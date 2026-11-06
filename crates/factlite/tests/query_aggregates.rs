//! Aggregates over distinct projected tuples, checked against independent
//! calculations from the committed data.

use std::collections::{BTreeMap, BTreeSet};

use factlite::{
    Aggregate, AggregateFn, AttributeDefinition, Cardinality, Direction, ErrorCode, Order, Params,
    Pattern, Query, QueryName, QueryResult, Store, TempId, Term, Tx, Uniqueness, Value, ValueType,
};
use proptest::prelude::*;

fn name(s: &str) -> QueryName {
    QueryName::new(s).unwrap()
}
fn var(s: &str) -> Term {
    Term::Var(name(s))
}
fn pattern(e: &str, a: &str, v: &str) -> Pattern {
    Pattern {
        entity: var(e),
        attribute: a.parse().unwrap(),
        value: var(v),
    }
}
fn agg(function: AggregateFn, v: &str, n: &str) -> Aggregate {
    Aggregate {
        function,
        var: name(v),
        name: name(n),
    }
}
fn definition(a: &str, ty: ValueType, cardinality: Cardinality) -> AttributeDefinition {
    AttributeDefinition::new(a.parse().unwrap(), ty, cardinality, Uniqueness::None).unwrap()
}

/// Runs of (recipe, cost, tags); returns the store and the snapshot.
fn store_with(runs: &[(u8, Value, Vec<u8>)], cost: ValueType) -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("agg.sqlite")).unwrap();
    let mut tx = Tx::builder()
        .define_attribute(definition(
            "run/recipe",
            ValueType::String,
            Cardinality::One,
        ))
        .define_attribute(definition("run/cost", cost, Cardinality::One))
        .define_attribute(definition("run/tag", ValueType::String, Cardinality::Many));
    for (index, (recipe, cost, tags)) in runs.iter().enumerate() {
        let label = TempId::new(format!("r{index}")).unwrap();
        tx = tx
            .new_entity(label.clone())
            .assert(
                label.clone(),
                "run/recipe".parse().unwrap(),
                Value::String(format!("recipe-{recipe}")),
            )
            .assert(label.clone(), "run/cost".parse().unwrap(), cost.clone());
        for tag in tags {
            tx = tx.assert(
                label.clone(),
                "run/tag".parse().unwrap(),
                Value::String(format!("t{tag}")),
            );
        }
    }
    store.commit(tx.build().unwrap()).unwrap();
    (dir, store)
}

fn rows_as_set(result: &QueryResult) -> BTreeSet<String> {
    result.rows.iter().map(|row| format!("{row:?}")).collect()
}

fn per_recipe(with_run: bool) -> Query {
    Query {
        patterns: vec![
            pattern("run", "run/recipe", "recipe"),
            pattern("run", "run/cost", "cost"),
        ],
        select: vec![name("recipe")],
        with: if with_run {
            vec![name("run")]
        } else {
            Vec::new()
        },
        aggregates: vec![
            agg(AggregateFn::Count, "cost", "n"),
            agg(AggregateFn::CountDistinct, "cost", "distinct"),
            agg(AggregateFn::Sum, "cost", "total"),
            agg(AggregateFn::Min, "cost", "low"),
            agg(AggregateFn::Max, "cost", "high"),
        ],
        ..Query::default()
    }
}

#[test]
fn per_group_aggregates_count_distinct_tuples_unless_with_keeps_matches_apart() {
    let i = Value::I64;
    let runs = vec![
        (0, i(5), vec![]),
        (0, i(5), vec![]),
        (0, i(7), vec![]),
        (1, i(-2), vec![]),
    ];
    let (_dir, store) = store_with(&runs, ValueType::I64);
    let db = store.db().unwrap();
    let s = |n: u8| Value::String(format!("recipe-{n}"));
    let with = db.query(&per_recipe(true), &Params::new()).unwrap();
    assert_eq!(
        with.columns,
        ["recipe", "n", "distinct", "total", "low", "high"]
            .map(name)
            .to_vec()
    );
    assert_eq!(
        rows_as_set(&with),
        rows_as_set(&QueryResult {
            columns: vec![],
            rows: vec![
                vec![s(0), i(3), i(2), i(17), i(5), i(7)],
                vec![s(1), i(1), i(1), i(-2), i(-2), i(-2)],
            ],
        })
    );
    // Without `with`, the two equal costs of recipe-0 are one tuple.
    let without = db.query(&per_recipe(false), &Params::new()).unwrap();
    assert_eq!(
        rows_as_set(&without),
        rows_as_set(&QueryResult {
            columns: vec![],
            rows: vec![
                vec![s(0), i(2), i(2), i(12), i(5), i(7)],
                vec![s(1), i(1), i(1), i(-2), i(-2), i(-2)],
            ],
        })
    );
}

#[test]
fn global_aggregates_ordering_limits_history_and_empty_matches() {
    let i = Value::I64;
    let runs: Vec<_> = (0..6)
        .map(|n| (n % 3, i(i64::from(n) * 10), vec![]))
        .collect();
    let (_dir, store) = store_with(&runs, ValueType::I64);
    let first = store.head().unwrap();
    let db = store.db().unwrap();
    let global = Query {
        patterns: vec![pattern("run", "run/cost", "cost")],
        with: vec![name("run")],
        aggregates: vec![
            agg(AggregateFn::Count, "run", "runs"),
            agg(AggregateFn::Sum, "cost", "total"),
        ],
        ..Query::default()
    };
    assert_eq!(
        db.query(&global, &Params::new()).unwrap().rows,
        vec![vec![i(6), i(150)]]
    );
    // Order by an aggregate, descending, then limit.
    let ranked = Query {
        order: vec![Order {
            field: name("total"),
            direction: Direction::Desc,
        }],
        limit: Some(2),
        ..per_recipe(true)
    };
    let rows = db.query(&ranked, &Params::new()).unwrap().rows;
    assert_eq!(
        rows.iter().map(|row| row[3].clone()).collect::<Vec<_>>(),
        vec![i(70), i(50)]
    );
    // No matching tuples yield no rows, even without grouping.
    let none = Query {
        patterns: vec![
            pattern("run", "run/cost", "cost"),
            Pattern {
                entity: var("run"),
                attribute: "run/tag".parse().unwrap(),
                value: var("tag"),
            },
        ],
        aggregates: vec![agg(AggregateFn::Count, "run", "runs")],
        ..Query::default()
    };
    assert!(db.query(&none, &Params::new()).unwrap().rows.is_empty());
    // A later commit does not change the earlier view's aggregates.
    store
        .commit(
            Tx::builder()
                .new_entity(TempId::new("late").unwrap())
                .assert(
                    TempId::new("late").unwrap(),
                    "run/cost".parse().unwrap(),
                    i(1000),
                )
                .build()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(
        store
            .db_at(&first)
            .unwrap()
            .query(&global, &Params::new())
            .unwrap()
            .rows,
        vec![vec![i(6), i(150)]]
    );
    assert_eq!(
        store
            .db()
            .unwrap()
            .query(&global, &Params::new())
            .unwrap()
            .rows,
        vec![vec![i(7), i(1150)]]
    );
    // A zero limit returns the aggregate columns with no rows.
    let zero = db
        .query(
            &Query {
                limit: Some(0),
                ..global
            },
            &Params::new(),
        )
        .unwrap();
    assert_eq!(
        (zero.columns, zero.rows),
        (vec![name("runs"), name("total")], vec![])
    );
}

#[test]
fn min_max_order_strings_and_refs_and_sums_fail_rather_than_wrap() {
    let runs = vec![
        (0, Value::I64(i64::MAX), vec![1, 2]),
        (0, Value::I64(1), vec![3]),
    ];
    let (_dir, store) = store_with(&runs, ValueType::I64);
    let db = store.db().unwrap();
    let tags = Query {
        patterns: vec![Pattern {
            entity: var("run"),
            attribute: "run/tag".parse().unwrap(),
            value: var("tag"),
        }],
        aggregates: vec![
            agg(AggregateFn::Min, "tag", "first"),
            agg(AggregateFn::Max, "tag", "last"),
            agg(AggregateFn::Min, "run", "lowest"),
            agg(AggregateFn::CountDistinct, "run", "runs"),
        ],
        ..Query::default()
    };
    let row = &db.query(&tags, &Params::new()).unwrap().rows[0];
    assert_eq!(
        row[..2],
        [Value::String("t1".into()), Value::String("t3".into())]
    );
    assert!(matches!(row[2], Value::Ref(_)));
    assert_eq!(row[3], Value::I64(2));
    let overflow = db.query(&per_recipe(true), &Params::new()).unwrap_err();
    assert_eq!(overflow.code(), ErrorCode::InvalidRequest);
    let (_dir, store) = store_with(
        &[
            (0, Value::f64(f64::MAX).unwrap(), vec![]),
            (0, Value::f64(f64::MAX).unwrap(), vec![]),
        ],
        ValueType::F64,
    );
    assert_eq!(
        store
            .db()
            .unwrap()
            .query(&per_recipe(true), &Params::new())
            .unwrap_err()
            .code(),
        ErrorCode::InvalidRequest
    );
}

#[test]
fn aggregate_queries_are_validated_before_execution() {
    let (_dir, store) = store_with(&[(0, Value::I64(1), vec![])], ValueType::I64);
    let db = store.db().unwrap();
    let base = per_recipe(true);
    let code = |query: Query| db.query(&query, &Params::new()).unwrap_err().code();
    assert_eq!(
        code(Query {
            aggregates: vec![agg(AggregateFn::Sum, "recipe", "x")],
            ..base.clone()
        }),
        ErrorCode::TypeMismatch
    );
    for invalid in [
        Query {
            aggregates: vec![agg(AggregateFn::Count, "nope", "x")],
            ..base.clone()
        },
        Query {
            aggregates: vec![],
            ..base.clone()
        },
        Query {
            with: vec![name("recipe")],
            ..base.clone()
        },
        Query {
            with: vec![name("run"), name("run")],
            ..base.clone()
        },
        Query {
            with: vec![name("nope")],
            ..base.clone()
        },
        Query {
            aggregates: vec![agg(AggregateFn::Count, "cost", "recipe")],
            ..base.clone()
        },
        Query {
            aggregates: vec![
                agg(AggregateFn::Count, "cost", "a"),
                agg(AggregateFn::Max, "cost", "a"),
            ],
            ..base.clone()
        },
        Query {
            order: vec![Order {
                field: name("nope"),
                direction: Direction::Asc,
            }],
            ..base.clone()
        },
    ] {
        assert_eq!(
            code(invalid.clone()),
            ErrorCode::InvalidRequest,
            "{invalid:?}"
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]
    #[test]
    fn grouped_aggregates_equal_an_independent_calculation(
        runs in proptest::collection::vec((0_u8..3, -50_i64..50, proptest::collection::vec(0_u8..3, 0..3)), 0..16),
        floats in any::<bool>(),
        with_run in any::<bool>(),
    ) {
        let cost = |n: i64| if floats { Value::f64(n as f64 / 4.0).unwrap() } else { Value::I64(n) };
        let data: Vec<_> = runs.iter().map(|(r, c, t)| (*r, cost(*c), t.clone())).collect();
        let (_dir, store) = store_with(&data, if floats { ValueType::F64 } else { ValueType::I64 });
        let actual = store.db().unwrap().query(&per_recipe(with_run), &Params::new()).unwrap();
        // Independent calculation: distinct (recipe, run?, cost) tuples per recipe.
        let mut groups: BTreeMap<u8, BTreeSet<(Option<usize>, i64)>> = BTreeMap::new();
        for (index, (recipe, cost, _)) in runs.iter().enumerate() {
            groups.entry(*recipe).or_default().insert((with_run.then_some(index), *cost));
        }
        let expected = QueryResult {
            columns: vec![],
            rows: groups
                .into_iter()
                .map(|(recipe, tuples)| {
                    let costs: Vec<i64> = tuples.iter().map(|(_, c)| *c).collect();
                    let distinct: BTreeSet<i64> = costs.iter().copied().collect();
                    let mut sorted = costs.clone();
                    sorted.sort();
                    let total = if floats {
                        Value::f64(sorted.iter().fold(0.0, |sum, c| sum + *c as f64 / 4.0)).unwrap()
                    } else {
                        Value::I64(costs.iter().sum())
                    };
                    vec![
                        Value::String(format!("recipe-{recipe}")),
                        Value::I64(costs.len() as i64),
                        Value::I64(distinct.len() as i64),
                        total,
                        cost(*sorted.first().unwrap()),
                        cost(*sorted.last().unwrap()),
                    ]
                })
                .collect(),
        };
        prop_assert_eq!(rows_as_set(&actual), rows_as_set(&expected));
    }
}
