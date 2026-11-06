//! Bound SQL over a temporary relation built from the pinned state, never head.
use std::collections::{BTreeMap, HashSet};

use rusqlite::{Transaction, params, params_from_iter, types::Value as SqlValue};

use crate::{
    AttrName, Comparison, Db, Direction, Fact, Params, Query, QueryName, QueryResult, Result,
    Schema, Term, Value, ValueType, codec,
};

struct Compiled {
    sql: String,
    bindings: Vec<SqlValue>,
    columns: Vec<ValueType>,
}
struct Compiler<'a> {
    params: &'a Params,
    variables: BTreeMap<QueryName, String>,
    bindings: Vec<SqlValue>,
}
impl Compiler<'_> {
    fn bind(&mut self, value: SqlValue) -> String {
        self.bindings.push(value);
        format!("?{}", self.bindings.len())
    }
    fn term(&mut self, term: &Term) -> String {
        match term {
            Term::Var(name) => self.variables[name].clone(),
            Term::Const(value) => self.bind(SqlValue::Blob(codec::encode_value(value))),
            Term::Param(name) => self.bind(SqlValue::Blob(codec::encode_value(&self.params[name]))),
        }
    }
    fn position(&mut self, term: &Term, expression: String, conditions: &mut Vec<String>) {
        if let Term::Var(name) = term
            && !self.variables.contains_key(name)
        {
            self.variables.insert(name.clone(), expression);
            return;
        }
        conditions.push(format!("{expression} = {}", self.term(term)));
    }
}
fn attribute_ids(schema: &Schema) -> BTreeMap<AttrName, i64> {
    schema
        .attributes()
        .enumerate()
        .map(|(index, definition)| (definition.name().clone(), index as i64))
        .collect()
}
fn compile(query: &Query, params: &Params, schema: &Schema) -> Result<Compiled> {
    let types = query.types(schema, params)?;
    let attrs = attribute_ids(schema);
    let mut compiler = Compiler {
        params,
        variables: BTreeMap::new(),
        bindings: Vec::new(),
    };
    let mut conditions = Vec::new();
    let mut from = Vec::new();
    for (index, pattern) in query.patterns.iter().enumerate() {
        let alias = format!("f{index}");
        from.push(format!("temp.factlite_query_facts AS {alias}"));
        let attribute = compiler.bind(SqlValue::Integer(attrs[&pattern.attribute]));
        conditions.push(format!("{alias}.attribute = {attribute}"));
        compiler.position(&pattern.entity, format!("{alias}.entity"), &mut conditions);
        compiler.position(&pattern.value, format!("{alias}.value"), &mut conditions);
    }
    let term_type = |term: &Term| match term {
        Term::Var(name) => types[name],
        Term::Const(value) => value.value_type(),
        Term::Param(name) => query.inputs[name],
    };
    for predicate in &query.predicates {
        // BLOB equality alone is not typed equality: e.g. empty string != empty bytes.
        if term_type(&predicate.left) != term_type(&predicate.right) {
            conditions.push(
                if predicate.comparison == Comparison::Ne {
                    "1"
                } else {
                    "0"
                }
                .into(),
            );
        } else {
            let left = compiler.term(&predicate.left);
            let right = compiler.term(&predicate.right);
            let op = match predicate.comparison {
                Comparison::Eq => "=",
                Comparison::Ne => "!=",
                Comparison::Lt => "<",
                Comparison::Le => "<=",
                Comparison::Gt => ">",
                Comparison::Ge => ">=",
            };
            conditions.push(format!("{left} {op} {right}"));
        }
    }
    let selected: Vec<_> = query
        .select
        .iter()
        .map(|name| compiler.variables[name].clone())
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
    Ok(Compiled {
        sql,
        bindings: compiler.bindings,
        columns: query.select.iter().map(|name| types[name]).collect(),
    })
}

pub(crate) fn evaluate(
    transaction: &Transaction<'_>,
    query: &Query,
    params: &Params,
    schema: &Schema,
    facts: &HashSet<Fact>,
) -> Result<QueryResult> {
    query.validate(schema, params)?;
    // Avoid imposing SQLite's build-specific join/expression/column limits on
    // otherwise valid ASTs. The reference path remains a semantic fallback.
    if query.patterns.len() > 32
        || query.predicates.len() > 128
        || query.select.len() > 256
        || query.order.len() > 256
    {
        return crate::query_eval::evaluate(query, params, schema, facts);
    }
    let compiled = compile(query, params, schema)?;
    if query.limit == Some(0) {
        return Ok(QueryResult {
            columns: query.select.clone(),
            rows: Vec::new(),
        });
    }
    transaction.execute_batch("CREATE TEMP TABLE factlite_query_facts(attribute INTEGER NOT NULL, entity BLOB NOT NULL, value BLOB NOT NULL, PRIMARY KEY(attribute,entity,value)) WITHOUT ROWID;")?;
    let attrs = attribute_ids(schema);
    {
        let mut insert = transaction.prepare(
            "INSERT INTO temp.factlite_query_facts(attribute,entity,value) VALUES (?1,?2,?3)",
        )?;
        for fact in facts {
            insert.execute(params![
                attrs[&fact.attribute],
                codec::encode_value(&Value::Ref(fact.entity.clone())),
                codec::encode_value(&fact.value)
            ])?;
        }
    }
    let mut output = Vec::new();
    {
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
    }
    transaction.execute_batch("DROP TABLE temp.factlite_query_facts")?;
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
        self.read(|transaction, model| {
            evaluate(
                transaction,
                query,
                params,
                &model.catalog.schema_at(self.snapshot().revision())?,
                &model.facts,
            )
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
            let actual = evaluate(transaction, q, params, &schema, &model.facts)?;
            let expected = crate::query_eval::evaluate(q, params, &schema, &model.facts)?;
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
            assert_eq!(count, 0, "temporary relation cleaned up");
            Ok(actual)
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
        let compiled = compile(&q, &Params::new(), &store.db().unwrap().schema().unwrap()).unwrap();
        assert_eq!(
            compiled.sql,
            "SELECT DISTINCT f0.value FROM temp.factlite_query_facts AS f0 WHERE f0.attribute = ?1 AND f0.entity = ?2 AND f0.value = ?3 ORDER BY 1 ASC LIMIT ?4"
        );
        assert!(matches!(compiled.bindings[0], SqlValue::Integer(0)));
        assert!(matches!(compiled.bindings[3], SqlValue::Integer(-1)));
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
    fn temp_failure_rolls_back_and_leaves_connection_and_historical_queries_usable() {
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
                AuthAction::Insert {
                    table_name: "factlite_query_facts",
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
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1024))]
        #[test]
        fn typed_graph_joins_predicates_order_limits_and_history_match_reference(
            ty in 0_usize..6, samples in proptest::collection::vec((any::<bool>(),any::<i64>(),any::<u64>(),any::<String>()),0..5),
            edges in proptest::collection::vec((0_usize..3,0_usize..3),0..8), op in 0_usize..6, descending in any::<bool>(),limit in 0_u64..20,
        ) {
            let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("graph.sqlite"); let store = Store::open(&path).unwrap();
            let labels: Vec<_> = (0..3).map(|i| TempId::new(i.to_string()).unwrap()).collect();
            let mut tx = Tx::builder(); for label in &labels { tx = tx.new_entity(label.clone()); }
            let allocated = store.commit(tx.build().unwrap()).unwrap(); let ids = allocated.created_entities;
            let domain = [ValueType::Bool,ValueType::I64,ValueType::F64,ValueType::String,ValueType::Bytes,ValueType::Ref][ty];
            let mut values: Vec<_> = samples.into_iter().map(|(b,i,bits,s)| match ty {
                0 => Value::Bool(b),1 => Value::I64(i),2 => Value::f64(f64::from_bits(bits)).unwrap_or_else(|_| Value::f64(0.0).unwrap()),3 => Value::String(s),4 => Value::Bytes(s.into_bytes()),5 => Value::Ref(ids[usize::from(b)].clone()),_ => unreachable!()
            }).collect();
            let wanted = values.first().cloned().unwrap_or_else(|| match ty {0 => Value::Bool(false),1 => Value::I64(0),2 => Value::f64(0.0).unwrap(),3 => Value::String(String::new()),4 => Value::Bytes(Vec::new()),_ => Value::Ref(ids[0].clone())});
            values.push(wanted.clone());
            let mut tx = Tx::builder().define_attribute(definition("x/value",domain)).define_attribute(definition("x/link",ValueType::Ref));
            for (i,value) in values.iter().enumerate() { tx = tx.assert(ids[i%3].clone(),"x/value".parse().unwrap(),value.clone()); }
            for (a,b) in edges { tx = tx.assert(ids[a].clone(),"x/link".parse().unwrap(),ValueTerm::Value(Value::Ref(ids[b].clone()))); }
            let a = store.commit(tx.build().unwrap()).unwrap(); let db_a = store.db().unwrap();
            let direction = if descending {Direction::Desc} else {Direction::Asc};
            let q = Query { inputs:[(name("wanted"),domain)].into(),patterns:vec![pattern(var("e"),"x/link",var("peer")),pattern(var("peer"),"x/value",var("v"))],predicates:vec![Predicate {left:var("v"),comparison:[Comparison::Eq,Comparison::Ne,Comparison::Lt,Comparison::Le,Comparison::Gt,Comparison::Ge][op],right:Term::Param(name("wanted"))}],select:vec![name("v"),name("e"),name("peer")],order:vec![order("v",direction),order("e",direction),order("peer",direction)],limit:Some(limit) };
            let params = [(name("wanted"),wanted)].into(); let expected = parity(&db_a,&q,&params);
            store.commit(Tx::builder().define_attribute(definition("later/value",ValueType::Bool)).assert(ids[0].clone(),"x/value".parse().unwrap(),values[0].clone()).build().unwrap()).unwrap();
            prop_assert_eq!(parity(&db_a,&q,&params),expected.clone());
            drop((store,db_a));let reopened = Store::open(&path).unwrap();
            prop_assert_eq!(parity(&reopened.db_at(&a.after).unwrap(),&q,&params),expected);
        }
    }
}
