//! Correctness-first conjunctive evaluation over a replayed historical fact set.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashSet},
};

use crate::{
    Comparison, Db, Direction, Fact, Params, Query, QueryName, QueryResult, Result, Schema, Term,
    Value,
};

type Binding = BTreeMap<QueryName, Value>;

fn unify(term: &Term, value: &Value, binding: &mut Binding, params: &Params) -> bool {
    match term {
        Term::Const(expected) => expected == value,
        Term::Param(name) => params.get(name).expect("validated input") == value,
        Term::Var(name) => match binding.get(name) {
            Some(expected) => expected == value,
            None => {
                binding.insert(name.clone(), value.clone());
                true
            }
        },
    }
}
fn resolve<'a>(term: &'a Term, binding: &'a Binding, params: &'a Params) -> &'a Value {
    match term {
        Term::Const(value) => value,
        Term::Var(name) => binding.get(name).expect("validated bound variable"),
        Term::Param(name) => params.get(name).expect("validated input"),
    }
}
fn compare(left: &Value, comparison: Comparison, right: &Value) -> Result<bool> {
    Ok(match comparison {
        Comparison::Eq => left == right,
        Comparison::Ne => left != right,
        Comparison::Lt => left.compare(right)? == Ordering::Less,
        Comparison::Le => left.compare(right)? != Ordering::Greater,
        Comparison::Gt => left.compare(right)? == Ordering::Greater,
        Comparison::Ge => left.compare(right)? != Ordering::Less,
    })
}

pub(crate) fn evaluate(
    query: &Query,
    params: &Params,
    schema: &Schema,
    facts: &HashSet<Fact>,
) -> Result<QueryResult> {
    query.validate(schema, params)?;
    let empty = || QueryResult {
        columns: query.select.clone(),
        rows: Vec::new(),
    };
    if query.limit == Some(0) {
        return Ok(empty());
    }
    let mut bindings = vec![Binding::new()];
    for pattern in &query.patterns {
        let candidates: Vec<_> = facts
            .iter()
            .filter(|fact| fact.attribute == pattern.attribute)
            .collect();
        let mut next = Vec::new();
        for binding in bindings {
            for fact in &candidates {
                let mut joined = binding.clone();
                if unify(
                    &pattern.entity,
                    &Value::Ref(fact.entity.clone()),
                    &mut joined,
                    params,
                ) && unify(&pattern.value, &fact.value, &mut joined, params)
                {
                    next.push(joined);
                }
            }
        }
        bindings = next;
        if bindings.is_empty() {
            return Ok(empty());
        }
    }
    let mut distinct = HashSet::new();
    for binding in bindings {
        let mut matches = true;
        for predicate in &query.predicates {
            if !compare(
                resolve(&predicate.left, &binding, params),
                predicate.comparison,
                resolve(&predicate.right, &binding, params),
            )? {
                matches = false;
                break;
            }
        }
        if matches {
            distinct.insert(
                query
                    .select
                    .iter()
                    .map(|name| binding.get(name).expect("validated selection").clone())
                    .collect::<Vec<_>>(),
            );
        }
    }
    let mut rows: Vec<_> = distinct.into_iter().collect();
    let order: Vec<_> = query
        .order
        .iter()
        .map(|order| {
            (
                query
                    .select
                    .iter()
                    .position(|name| name == &order.field)
                    .expect("validated selected order field"),
                order.direction,
            )
        })
        .collect();
    rows.sort_by(|left, right| {
        for (index, direction) in &order {
            let ordering = left[*index]
                .compare(&right[*index])
                .expect("validated uniform column domain");
            let ordering = match direction {
                Direction::Asc => ordering,
                Direction::Desc => ordering.reverse(),
            };
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        Ordering::Equal
    });
    if let Some(limit) = query.limit {
        rows.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    }
    Ok(QueryResult {
        columns: query.select.clone(),
        rows,
    })
}

impl Db {
    /// Match/filter/project/distinct/order/limit against this immutable view.
    /// Every result is owned; entity columns are typed references. Include a
    /// total ordering for predictable limits; unordered rows have unspecified order.
    pub fn query(&self, query: &Query, params: &Params) -> Result<QueryResult> {
        self.read(|_, model| {
            evaluate(
                query,
                params,
                &model.catalog.schema_at(self.snapshot().revision())?,
                &model.facts,
            )
        })
    }
}
