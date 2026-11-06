//! Bound SQL over the revision-interval datom index, bounded to the pinned
//! revision, never head.
use std::collections::BTreeMap;

use rusqlite::{Transaction, params_from_iter, types::Value as SqlValue};

use crate::{
    Comparison, Db, Direction, Params, Predicate, Query, QueryName, QueryResult, Result, Schema,
    SnapshotId, Term, Value, ValueType, codec,
};

struct Compiled {
    sql: String,
    bindings: Vec<SqlValue>,
    columns: Vec<ValueType>,
}

/// SQL for a bound variable. Every variable has a BLOB form, matching value
/// columns and decoding. A variable first bound at an entity position also
/// keeps the TEXT entity column, so later entity positions join on it without
/// a cast and can use the primary key.
#[derive(Clone)]
struct Binding {
    blob: String,
    text: Option<String>,
}

impl Binding {
    fn text(&self) -> String {
        self.text
            .clone()
            .unwrap_or_else(|| format!("CAST({} AS TEXT)", self.blob))
    }
}

#[derive(Clone, Copy)]
enum Position {
    Entity,
    Value,
}

struct Compiler<'a> {
    params: &'a Params,
    variables: BTreeMap<QueryName, Binding>,
    bindings: Vec<SqlValue>,
}
impl Compiler<'_> {
    fn bind(&mut self, value: SqlValue) -> String {
        self.bindings.push(value);
        format!("?{}", self.bindings.len())
    }
    fn constant(&self, term: &Term) -> Option<Value> {
        match term {
            Term::Var(_) => None,
            Term::Const(value) => Some(value.clone()),
            Term::Param(name) => Some(self.params[name].clone()),
        }
    }
    fn term(&mut self, term: &Term) -> String {
        match term {
            Term::Var(name) => self.variables[name].blob.clone(),
            other => {
                let value = self.constant(other).expect("constant term");
                self.bind(SqlValue::Blob(codec::encode_value(&value)))
            }
        }
    }
    /// Adds one pattern's conditions on `alias`, restricted to rows of the
    /// attribute's type visible at the basis (?1).
    fn pattern(
        &mut self,
        pattern: &crate::Pattern,
        alias: &str,
        schema: &Schema,
        conditions: &mut Vec<String>,
    ) {
        let attribute = self.bind(SqlValue::Text(pattern.attribute.as_str().to_owned()));
        conditions.push(format!("{alias}.attribute = {attribute}"));
        // The stored tag must be the attribute's type, so a mistyped row (only
        // possible in a corrupted file) never matches and is never
        // reinterpreted as the schema type when decoded.
        let tag = self.bind(SqlValue::Integer(i64::from(codec::type_tag(
            schema
                .get(&pattern.attribute)
                .expect("validated attribute")
                .value_type(),
        ))));
        conditions.push(format!("{alias}.type_tag = {tag}"));
        conditions.push(format!(
            "{alias}.asserted <= ?1 AND ({alias}.retracted IS NULL OR {alias}.retracted > ?1)"
        ));
        self.position(&pattern.entity, alias, Position::Entity, conditions);
        self.position(&pattern.value, alias, Position::Value, conditions);
    }
    /// One comparison, given the types of its two sides.
    fn predicate(&mut self, predicate: &Predicate, types: (ValueType, ValueType)) -> String {
        // BLOB equality alone is not typed equality: e.g. empty string != empty bytes.
        if types.0 != types.1 {
            return if predicate.comparison == Comparison::Ne {
                "1"
            } else {
                "0"
            }
            .into();
        }
        let left = self.term(&predicate.left);
        let right = self.term(&predicate.right);
        let op = match predicate.comparison {
            Comparison::Eq => "=",
            Comparison::Ne => "!=",
            Comparison::Lt => "<",
            Comparison::Le => "<=",
            Comparison::Gt => ">",
            Comparison::Ge => ">=",
        };
        format!("{left} {op} {right}")
    }
    fn position(
        &mut self,
        term: &Term,
        alias: &str,
        position: Position,
        conditions: &mut Vec<String>,
    ) {
        let column = match position {
            Position::Entity => format!("{alias}.entity"),
            Position::Value => format!("{alias}.value"),
        };
        if let Term::Var(name) = term {
            match self.variables.get(name).cloned() {
                None => {
                    let binding = match position {
                        Position::Entity => Binding {
                            blob: format!("CAST({column} AS BLOB)"),
                            text: Some(column),
                        },
                        Position::Value => Binding {
                            blob: column,
                            text: None,
                        },
                    };
                    self.variables.insert(name.clone(), binding);
                }
                Some(binding) => conditions.push(match position {
                    Position::Entity => format!("{column} = {}", binding.text()),
                    Position::Value => format!("{column} = {}", binding.blob),
                }),
            }
            return;
        }
        let value = self.constant(term).expect("constant term");
        let bound = match (position, &value) {
            // Entity columns hold canonical identity text.
            (Position::Entity, Value::Ref(id)) => self.bind(SqlValue::Text(id.to_string())),
            _ => self.bind(SqlValue::Blob(codec::encode_value(&value))),
        };
        conditions.push(format!("{column} = {bound}"));
    }
}

/// Compiles the query against `main.datoms`, each pattern restricted to rows
/// visible at `basis` (the SQL bound to ?1).
fn compile(
    query: &Query,
    params: &Params,
    schema: &Schema,
    basis: &SnapshotId,
) -> Result<Compiled> {
    let types = query.types(schema, params)?;
    let mut compiler = Compiler {
        params,
        variables: BTreeMap::new(),
        bindings: vec![SqlValue::Blob(
            crate::store::revision_bytes(basis.revision()).to_vec(),
        )],
    };
    let mut conditions = Vec::new();
    let mut from = Vec::new();
    for (index, pattern) in query.patterns.iter().enumerate() {
        let alias = format!("f{index}");
        from.push(format!("main.datoms AS {alias}"));
        compiler.pattern(pattern, &alias, schema, &mut conditions);
    }
    let term_type = |types: &BTreeMap<QueryName, ValueType>, term: &Term| match term {
        Term::Var(name) => types[name],
        Term::Const(value) => value.value_type(),
        Term::Param(name) => query.inputs[name],
    };
    for predicate in &query.predicates {
        let types = (
            term_type(&types, &predicate.left),
            term_type(&types, &predicate.right),
        );
        conditions.push(compiler.predicate(predicate, types));
    }
    // Each group is a correlated NOT EXISTS. Join variables (all outer
    // variables for `not`) refer to the outer aliases; locals bind to the
    // group's own aliases, which carry the same visibility conditions.
    for (group, negation) in query.negations.iter().enumerate() {
        let scope = query.negation_scope(negation, schema, &types)?;
        let outer = compiler.variables.clone();
        if let Some(join) = &negation.join {
            compiler.variables.retain(|name, _| join.contains(name));
        }
        let mut inner_from = Vec::new();
        let mut inner = Vec::new();
        for (index, pattern) in negation.patterns.iter().enumerate() {
            let alias = format!("n{group}_{index}");
            inner_from.push(format!("main.datoms AS {alias}"));
            compiler.pattern(pattern, &alias, schema, &mut inner);
        }
        for predicate in &negation.predicates {
            let types = (
                term_type(&scope, &predicate.left),
                term_type(&scope, &predicate.right),
            );
            inner.push(compiler.predicate(predicate, types));
        }
        compiler.variables = outer;
        let mut subquery = "SELECT 1".to_owned();
        if !inner_from.is_empty() {
            subquery.push_str(&format!(" FROM {}", inner_from.join(", ")));
        }
        subquery.push_str(&format!(" WHERE {}", inner.join(" AND ")));
        conditions.push(format!("NOT EXISTS ({subquery})"));
    }
    let selected: Vec<_> = query
        .select
        .iter()
        .map(|name| compiler.variables[name].blob.clone())
        .collect();
    let mut sql = format!(
        "SELECT DISTINCT {}",
        if selected.is_empty() {
            "1".into()
        } else {
            selected.join(", ")
        }
    );
    if !from.is_empty() {
        sql.push_str(&format!(" FROM {}", from.join(", ")));
    }
    if !conditions.is_empty() {
        sql.push_str(&format!(" WHERE {}", conditions.join(" AND ")));
    }
    if !query.order.is_empty() {
        let order: Vec<_> = query
            .order
            .iter()
            .map(|order| {
                let position = query
                    .select
                    .iter()
                    .position(|name| name == &order.field)
                    .expect("validated selected order")
                    + 1;
                format!(
                    "{position} {}",
                    match order.direction {
                        Direction::Asc => "ASC",
                        Direction::Desc => "DESC",
                    }
                )
            })
            .collect();
        sql.push_str(&format!(" ORDER BY {}", order.join(", ")));
    }
    // SQLite's result cardinality is bounded by i64::MAX. A larger u64 request
    // imposes no additional restriction; bind SQLite's documented unlimited -1.
    if let Some(limit) = query.limit {
        let bound = compiler.bind(SqlValue::Integer(i64::try_from(limit).unwrap_or(-1)));
        sql.push_str(&format!(" LIMIT {bound}"));
    }
    // A query with no patterns never references ?1; keep it bound anyway.
    if from.is_empty() {
        sql = format!("SELECT * FROM ({sql}) WHERE ?1 IS NOT NULL");
    }
    Ok(Compiled {
        sql,
        bindings: compiler.bindings,
        columns: query.select.iter().map(|name| types[name]).collect(),
    })
}

/// Runs the compiled query in the caller's SQL snapshot. Large ASTs beyond the
/// conservative SQL budget load the state at `basis` for the reference
/// evaluator instead.
pub(crate) fn evaluate(
    transaction: &Transaction<'_>,
    query: &Query,
    params: &Params,
    schema: &Schema,
    basis: &SnapshotId,
) -> Result<QueryResult> {
    query.validate(schema, params)?;
    // Avoid imposing SQLite's build-specific join/expression/column limits on
    // otherwise valid ASTs. The reference path remains a semantic fallback.
    let group_patterns: usize = query.negations.iter().map(|n| n.patterns.len()).sum();
    let group_predicates: usize = query.negations.iter().map(|n| n.predicates.len()).sum();
    if query.patterns.len() + group_patterns > 32
        || query.predicates.len() + group_predicates > 128
        || query.select.len() > 256
        || query.order.len() > 256
    {
        let model = crate::index::model_at(transaction, basis)?;
        return crate::query_eval::evaluate(query, params, schema, model.facts());
    }
    if query.limit == Some(0) {
        return Ok(QueryResult {
            columns: crate::query_aggregate::result_columns(query),
            rows: Vec::new(),
        });
    }
    if !query.aggregates.is_empty() {
        // SQL produces the distinct tuples; grouping and arithmetic are shared
        // with the reference evaluator.
        let tuples = evaluate(
            transaction,
            &crate::query_aggregate::tuple_query(query),
            params,
            schema,
            basis,
        )?;
        return crate::query_aggregate::finish(query, params, tuples.rows);
    }
    let compiled = compile(query, params, schema, basis)?;
    let mut output = Vec::new();
    let mut statement = transaction.prepare(&compiled.sql)?;
    let mut rows = statement.query(params_from_iter(&compiled.bindings))?;
    while let Some(row) = rows.next()? {
        let values = compiled
            .columns
            .iter()
            .enumerate()
            .map(|(index, ty)| {
                codec::decode_value(codec::type_tag(*ty), &row.get::<_, Vec<u8>>(index)?)
            })
            .collect::<Result<_>>()?;
        output.push(values);
    }
    Ok(QueryResult {
        columns: query.select.clone(),
        rows: output,
    })
}

impl Db {
    /// Execute typed joined SQL against this immutable historical view. Every
    /// result is owned. Supply a total ordering for predictable limited results.
    /// Large ASTs beyond the conservative SQL budget use the reference evaluator.
    pub fn query(&self, query: &Query, params: &Params) -> Result<QueryResult> {
        self.read_basis(|transaction| {
            let schema = crate::index::schema_at(transaction, self.snapshot())?;
            evaluate(transaction, query, params, &schema, self.snapshot())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AttributeDefinition, Cardinality, Order, Pattern, Predicate, Store, TempId, Tx, Uniqueness,
        ValueTerm,
    };
    use proptest::prelude::*;
    use std::collections::HashSet;

    fn name(s: &str) -> QueryName {
        QueryName::new(s).unwrap()
    }
    fn var(s: &str) -> Term {
        Term::Var(name(s))
    }
    fn pattern(e: Term, a: &str, v: Term) -> Pattern {
        Pattern {
            entity: e,
            attribute: a.parse().unwrap(),
            value: v,
        }
    }
    fn definition(a: &str, ty: ValueType) -> AttributeDefinition {
        AttributeDefinition::new(a.parse().unwrap(), ty, Cardinality::Many, Uniqueness::None)
            .unwrap()
    }
    fn order(s: &str, direction: Direction) -> crate::Order {
        Order {
            field: name(s),
            direction,
        }
    }
    fn parity(db: &Db, q: &Query, params: &Params) -> QueryResult {
        db.read(|transaction, model| {
            let schema = model.catalog.schema_at(db.snapshot().revision())?;
            let actual = evaluate(transaction, q, params, &schema, db.snapshot())?;
            let expected = crate::query_eval::evaluate(q, params, &schema, model.facts())?;
            assert_eq!(actual.columns, expected.columns);
            if q.order.is_empty() && q.limit.is_none() {
                assert_eq!(
                    actual.rows.iter().collect::<HashSet<_>>(),
                    expected.rows.iter().collect::<HashSet<_>>()
                );
            } else {
                assert_eq!(actual, expected);
            }
            let count: i64 = transaction.query_row(
                "SELECT count(*) FROM sqlite_temp_master WHERE name='factlite_query_facts'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(count, 0, "queries create no temporary relation");
            Ok(actual)
        })
        .unwrap()
    }
    /// Like `parity`, but sum overflow may fail: both paths must then fail with
    /// the same code.
    fn parity_or_error(
        db: &Db,
        q: &Query,
        params: &Params,
    ) -> std::result::Result<QueryResult, crate::ErrorCode> {
        db.read(|transaction, model| {
            let schema = model.catalog.schema_at(db.snapshot().revision())?;
            let actual = evaluate(transaction, q, params, &schema, db.snapshot());
            let expected = crate::query_eval::evaluate(q, params, &schema, model.facts());
            Ok(match (actual, expected) {
                (Ok(actual), Ok(expected)) => {
                    assert_eq!(actual, expected);
                    Ok(actual)
                }
                (Err(actual), Err(expected)) => {
                    assert_eq!(actual.code(), expected.code());
                    assert_eq!(actual.to_string(), expected.to_string());
                    Err(actual.code())
                }
                (actual, expected) => panic!("diverged: {actual:?} vs {expected:?}"),
            })
        })
        .unwrap()
    }
    #[test]
    fn compiler_only_emits_trusted_aliases_and_binds_attributes_values_and_limit() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("bind.sqlite")).unwrap();
        let attr = "sql/\";DROP--";
        let report = store
            .commit(
                Tx::builder()
                    .define_attribute(definition(attr, ValueType::String))
                    .new_entity(TempId::new("e").unwrap())
                    .assert(
                        TempId::new("e").unwrap(),
                        attr.parse().unwrap(),
                        Value::String("'; DROP TABLE transactions; --\0雪".into()),
                    )
                    .build()
                    .unwrap(),
            )
            .unwrap();
        let e = report.created_entities[0].clone();
        let q = Query {
            patterns: vec![pattern(
                Term::Const(Value::Ref(e)),
                attr,
                var("column');DROP"),
            )],
            predicates: vec![Predicate {
                left: var("column');DROP"),
                comparison: Comparison::Eq,
                right: Term::Const(Value::String("'; DROP TABLE transactions; --\0雪".into())),
            }],
            select: vec![name("column');DROP")],
            order: vec![order("column');DROP", Direction::Asc)],
            limit: Some(u64::MAX),
            ..Query::default()
        };
        let db = store.db().unwrap();
        let compiled = compile(&q, &Params::new(), &db.schema().unwrap(), db.snapshot()).unwrap();
        assert_eq!(
            compiled.sql,
            "SELECT DISTINCT f0.value FROM main.datoms AS f0 WHERE f0.attribute = ?2 AND f0.type_tag = ?3 AND f0.asserted <= ?1 AND (f0.retracted IS NULL OR f0.retracted > ?1) AND f0.entity = ?4 AND f0.value = ?5 ORDER BY 1 ASC LIMIT ?6"
        );
        assert!(matches!(&compiled.bindings[1], SqlValue::Text(name) if name == attr));
        assert!(matches!(compiled.bindings[2], SqlValue::Integer(3)));
        assert!(matches!(&compiled.bindings[3], SqlValue::Text(id) if id.starts_with("e1.")));
        assert!(matches!(compiled.bindings[5], SqlValue::Integer(-1)));
        assert_eq!(
            parity(&store.db().unwrap(), &q, &Params::new()).rows.len(),
            1
        );
        assert_eq!(store.head().unwrap(), report.after);
    }
    #[test]
    fn blob_collisions_do_not_erase_type_distinctions_and_sql_limits_preserve_valid_queries() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("domains.sqlite")).unwrap();
        let q = |op| Query {
            predicates: vec![Predicate {
                left: Term::Const(Value::String(String::new())),
                comparison: op,
                right: Term::Const(Value::Bytes(Vec::new())),
            }],
            ..Query::default()
        };
        assert!(
            parity(&store.db().unwrap(), &q(Comparison::Eq), &Params::new())
                .rows
                .is_empty()
        );
        assert_eq!(
            parity(&store.db().unwrap(), &q(Comparison::Ne), &Params::new()).rows,
            vec![vec![]]
        );
        let report = store
            .commit(
                Tx::builder()
                    .define_attribute(definition("x/v", ValueType::I64))
                    .new_entity(TempId::new("e").unwrap())
                    .assert(
                        TempId::new("e").unwrap(),
                        "x/v".parse().unwrap(),
                        Value::I64(1),
                    )
                    .build()
                    .unwrap(),
            )
            .unwrap();
        let q = Query {
            patterns: vec![pattern(var("e"), "x/v", var("v")); 65],
            select: vec![name("v")],
            order: vec![order("v", Direction::Asc)],
            ..Query::default()
        };
        assert_eq!(
            parity(&store.db().unwrap(), &q, &Params::new()).rows,
            vec![vec![Value::I64(1)]]
        );
        assert_eq!(store.head().unwrap(), report.after);
    }
    #[test]
    fn read_failure_leaves_connection_and_historical_queries_usable() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("failure.sqlite")).unwrap();
        store
            .commit(
                Tx::builder()
                    .define_attribute(definition("x/v", ValueType::I64))
                    .new_entity(TempId::new("e").unwrap())
                    .assert(
                        TempId::new("e").unwrap(),
                        "x/v".parse().unwrap(),
                        Value::I64(1),
                    )
                    .build()
                    .unwrap(),
            )
            .unwrap();
        let db = store.db().unwrap();
        let q = Query {
            patterns: vec![pattern(var("e"), "x/v", var("v"))],
            select: vec![name("v")],
            ..Query::default()
        };
        use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
        store
            .connection()
            .unwrap()
            .authorizer(Some(|context: AuthContext<'_>| match context.action {
                AuthAction::Read {
                    table_name: "datoms",
                    ..
                } => Authorization::Deny,
                _ => Authorization::Allow,
            }))
            .unwrap();
        assert_eq!(
            db.query(&q, &Params::new()).unwrap_err().code(),
            crate::ErrorCode::Storage
        );
        store
            .connection()
            .unwrap()
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
            .unwrap();
        assert_eq!(
            parity(&db, &q, &Params::new()).rows,
            vec![vec![Value::I64(1)]]
        );
        store.commit(Tx::default()).unwrap();
        assert_eq!(
            parity(&db, &q, &Params::new()).rows,
            vec![vec![Value::I64(1)]]
        );
    }
    fn negation(
        join: Option<&[&str]>,
        patterns: Vec<Pattern>,
        predicates: Vec<Predicate>,
    ) -> crate::Negation {
        crate::Negation {
            join: join.map(|names| names.iter().map(|n| name(n)).collect()),
            patterns,
            predicates,
        }
    }
    fn compare(left: Term, comparison: Comparison, right: Term) -> Predicate {
        Predicate {
            left,
            comparison,
            right,
        }
    }
    /// Queries over `x/link` (ref), `x/value` (i64) and `x/flag` (bool) that
    /// exercise every negation form, checked by parity at one view.
    fn negation_queries(limit: u64) -> Vec<Query> {
        let base = Query {
            inputs: [(name("cut"), ValueType::I64)].into(),
            patterns: vec![pattern(var("e"), "x/link", var("p"))],
            select: vec![name("e"), name("p")],
            order: vec![order("e", Direction::Asc), order("p", Direction::Asc)],
            limit: Some(limit),
            ..Query::default()
        };
        let flagged = |e: &str| pattern(var(e), "x/flag", Term::Const(Value::Bool(true)));
        let valued = |e: &str, v: &str| pattern(var(e), "x/value", var(v));
        vec![
            // `not` with all variables bound outside.
            Query {
                negations: vec![negation(None, vec![flagged("p")], vec![])],
                ..base.clone()
            },
            // `not-join`: absence of any value, and of a value above the cut.
            Query {
                negations: vec![negation(Some(&["p"]), vec![valued("p", "o")], vec![])],
                ..base.clone()
            },
            Query {
                negations: vec![negation(
                    Some(&["p"]),
                    vec![valued("p", "o")],
                    vec![compare(var("o"), Comparison::Gt, Term::Param(name("cut")))],
                )],
                ..base.clone()
            },
            // A local joined across two group patterns, and two conjunctive groups.
            Query {
                negations: vec![
                    negation(
                        Some(&["e"]),
                        vec![pattern(var("e"), "x/link", var("q")), flagged("q")],
                        vec![],
                    ),
                    negation(
                        None,
                        vec![],
                        vec![compare(var("e"), Comparison::Eq, var("p"))],
                    ),
                ],
                ..base.clone()
            },
            // Predicate-only `not` over an outer value, mixed types fold.
            Query {
                patterns: vec![pattern(var("e"), "x/link", var("p")), valued("p", "v")],
                negations: vec![
                    negation(
                        None,
                        vec![],
                        vec![compare(var("v"), Comparison::Le, Term::Param(name("cut")))],
                    ),
                    negation(
                        None,
                        vec![],
                        vec![compare(
                            Term::Const(Value::String(String::new())),
                            Comparison::Eq,
                            Term::Const(Value::Bytes(Vec::new())),
                        )],
                    ),
                ],
                select: vec![name("e"), name("v")],
                order: vec![order("e", Direction::Asc), order("v", Direction::Asc)],
                ..base.clone()
            },
            // Variables bound at value positions outside used at entity
            // positions inside, and the reverse: no link back from p to e.
            Query {
                negations: vec![negation(
                    None,
                    vec![pattern(var("p"), "x/link", var("e"))],
                    vec![],
                )],
                ..base.clone()
            },
            // Uncorrelated absence with no positive patterns and an empty join.
            Query {
                patterns: vec![],
                negations: vec![negation(Some(&[]), vec![flagged("x")], vec![])],
                select: vec![],
                order: vec![],
                ..base.clone()
            },
            // Aggregated: links per entity to peers that have no value.
            Query {
                select: vec![name("e")],
                with: vec![name("p")],
                aggregates: vec![crate::Aggregate {
                    function: crate::AggregateFn::Count,
                    var: name("p"),
                    name: name("n"),
                }],
                order: vec![order("e", Direction::Asc)],
                negations: vec![negation(Some(&["p"]), vec![valued("p", "o")], vec![])],
                ..base.clone()
            },
        ]
    }
    // A value of the same type, reasserted later on another entity.
    fn wanted_again(values: &[Value]) -> Value {
        values.last().cloned().expect("at least the wanted value")
    }
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1024))]
        #[test]
        fn typed_graph_joins_predicates_order_limits_and_history_match_reference(
            ty in 0_usize..8, samples in proptest::collection::vec((any::<bool>(),any::<i64>(),any::<u64>(),any::<String>()),0..5),
            edges in proptest::collection::vec((0_usize..3,0_usize..3),0..8), op in 0_usize..6, descending in any::<bool>(),limit in 0_u64..20,
        ) {
            let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("graph.sqlite"); let store = Store::open(&path).unwrap();
            let labels: Vec<_> = (0..3).map(|i| TempId::new(i.to_string()).unwrap()).collect();
            let mut tx = Tx::builder(); for label in &labels { tx = tx.new_entity(label.clone()); }
            let allocated = store.commit(tx.build().unwrap()).unwrap(); let ids = allocated.created_entities;
            let domain = [ValueType::Bool,ValueType::I64,ValueType::F64,ValueType::String,ValueType::Bytes,ValueType::Ref,ValueType::Keyword,ValueType::Inst][ty];
            let mut values: Vec<_> = samples.into_iter().map(|(b,i,bits,s)| match ty {
                0 => Value::Bool(b),1 => Value::I64(i),2 => Value::f64(f64::from_bits(bits)).unwrap_or_else(|_| Value::f64(0.0).unwrap()),3 => Value::String(s),4 => Value::Bytes(s.into_bytes()),5 => Value::Ref(ids[usize::from(b)].clone()),6 => Value::keyword(format!("k{}", i.rem_euclid(5))).unwrap(),7 => Value::Inst(i),_ => unreachable!()
            }).collect();
            let wanted = values.first().cloned().unwrap_or_else(|| match ty {0 => Value::Bool(false),1 => Value::I64(0),2 => Value::f64(0.0).unwrap(),3 => Value::String(String::new()),4 => Value::Bytes(Vec::new()),5 => Value::Ref(ids[0].clone()),6 => Value::keyword("k0").unwrap(),_ => Value::Inst(0)});
            values.push(wanted.clone());
            let mut tx = Tx::builder().define_attribute(definition("x/value",domain)).define_attribute(definition("x/link",ValueType::Ref));
            for (i,value) in values.iter().enumerate() { tx = tx.assert(ids[i%3].clone(),"x/value".parse().unwrap(),value.clone()); }
            for (a,b) in edges { tx = tx.assert(ids[a].clone(),"x/link".parse().unwrap(),ValueTerm::Value(Value::Ref(ids[b].clone()))); }
            let a = store.commit(tx.build().unwrap()).unwrap(); let db_a = store.db().unwrap();
            let direction = if descending {Direction::Desc} else {Direction::Asc};
            let q = Query { inputs:[(name("wanted"),domain)].into(),patterns:vec![pattern(var("e"),"x/link",var("peer")),pattern(var("peer"),"x/value",var("v"))],predicates:vec![Predicate {left:var("v"),comparison:[Comparison::Eq,Comparison::Ne,Comparison::Lt,Comparison::Le,Comparison::Gt,Comparison::Ge][op],right:Term::Param(name("wanted"))}],select:vec![name("v"),name("e"),name("peer")],order:vec![order("v",direction),order("e",direction),order("peer",direction)],limit:Some(limit), ..Query::default() };
            let params = [(name("wanted"),wanted)].into(); let expected = parity(&db_a,&q,&params);
            // The same join, aggregated per entity, through SQL tuples and the reference.
            let mut aggregates = vec![
                crate::Aggregate { function: crate::AggregateFn::Count, var: name("peer"), name: name("n") },
                crate::Aggregate { function: crate::AggregateFn::CountDistinct, var: name("v"), name: name("distinct") },
                crate::Aggregate { function: crate::AggregateFn::Min, var: name("v"), name: name("low") },
                crate::Aggregate { function: crate::AggregateFn::Max, var: name("v"), name: name("high") },
            ];
            if matches!(domain, ValueType::I64 | ValueType::F64) {
                aggregates.push(crate::Aggregate { function: crate::AggregateFn::Sum, var: name("v"), name: name("total") });
            }
            let grouped = Query { select: vec![name("e")], with: vec![name("peer")], aggregates, order: vec![order("e", direction)], limit: Some(limit), ..q.clone() };
            let grouped_expected = parity_or_error(&db_a,&grouped,&params);
            // Later commits retract and add values and links; A must not see them.
            store.commit(Tx::builder().define_attribute(definition("later/value",ValueType::Bool)).retract(ids[0].clone(),"x/value".parse().unwrap(),values[0].clone()).assert(ids[1].clone(),"x/link".parse().unwrap(),ValueTerm::Value(Value::Ref(ids[2].clone()))).build().unwrap()).unwrap();
            store.commit(Tx::builder().assert(ids[2].clone(),"x/value".parse().unwrap(),wanted_again(&values)).assert(ids[0].clone(),"x/value".parse().unwrap(),values[0].clone()).build().unwrap()).unwrap();
            prop_assert_eq!(parity(&db_a,&q,&params),expected.clone());
            prop_assert_eq!(parity_or_error(&db_a,&grouped,&params),grouped_expected.clone());
            drop((store,db_a));let reopened = Store::open(&path).unwrap();
            prop_assert_eq!(parity(&reopened.db_at(&a.after).unwrap(),&q,&params),expected);
            prop_assert_eq!(parity_or_error(&reopened.db_at(&a.after).unwrap(),&grouped,&params),grouped_expected);
        }
        #[test]
        fn negation_matches_reference_at_the_view_revision(
            links in proptest::collection::vec((0_usize..4, 0_usize..4), 0..8),
            values in proptest::collection::vec((0_usize..4, 0_i64..4), 0..6),
            flags in proptest::collection::vec(0_usize..4, 0..3),
            later in proptest::collection::vec((0_usize..3, 0_usize..4, 0_i64..4), 0..6),
            cut in 0_i64..4, limit in 1_u64..8,
        ) {
            let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("not.sqlite"); let store = Store::open(&path).unwrap();
            let mut tx = Tx::builder().define_attribute(definition("x/link", ValueType::Ref)).define_attribute(definition("x/value", ValueType::I64)).define_attribute(definition("x/flag", ValueType::Bool));
            for i in 0..4 { tx = tx.new_entity(TempId::new(i.to_string()).unwrap()); }
            let ids = store.commit(tx.build().unwrap()).unwrap().created_entities;
            let mut tx = Tx::builder();
            for (a, b) in &links { tx = tx.assert(ids[*a].clone(), "x/link".parse().unwrap(), ValueTerm::Value(Value::Ref(ids[*b].clone()))); }
            for (e, v) in &values { tx = tx.assert(ids[*e].clone(), "x/value".parse().unwrap(), Value::I64(*v)); }
            for e in &flags { tx = tx.assert(ids[*e].clone(), "x/flag".parse().unwrap(), Value::Bool(true)); }
            let a = store.commit(tx.build().unwrap()).unwrap();
            let db_a = store.db().unwrap();
            let params: Params = [(name("cut"), Value::I64(cut))].into();
            let queries = negation_queries(limit);
            let expected: Vec<_> = queries.iter().map(|q| parity(&db_a, q, &params)).collect();
            // Later commits retract facts A saw and assert facts it must not see.
            let mut tx = Tx::builder();
            for (kind, e, v) in &later {
                tx = match kind {
                    0 => tx.assert(ids[*e].clone(), "x/value".parse().unwrap(), Value::I64(*v)),
                    1 => tx.assert(ids[*e].clone(), "x/flag".parse().unwrap(), Value::Bool(true)),
                    _ => tx.assert(ids[*e].clone(), "x/link".parse().unwrap(), ValueTerm::Value(Value::Ref(ids[usize::try_from(*v).unwrap()].clone()))),
                };
            }
            store.commit(tx.build().unwrap()).unwrap();
            let mut tx = Tx::builder();
            for (e, v) in values.iter().take(2).collect::<std::collections::BTreeSet<_>>() { tx = tx.retract(ids[*e].clone(), "x/value".parse().unwrap(), Value::I64(*v)); }
            for e in flags.iter().take(1) { tx = tx.retract(ids[*e].clone(), "x/flag".parse().unwrap(), Value::Bool(true)); }
            store.commit(tx.build().unwrap()).unwrap();
            for (q, expected) in queries.iter().zip(&expected) {
                prop_assert_eq!(&parity(&db_a, q, &params), expected);
                prop_assert_eq!(&parity(&store.db().unwrap(), q, &params).columns, &expected.columns);
            }
            drop((store, db_a));
            let reopened = Store::open(&path).unwrap();
            for (q, expected) in queries.iter().zip(&expected) {
                prop_assert_eq!(&parity(&reopened.db_at(&a.after).unwrap(), q, &params), expected);
            }
        }
    }
}
