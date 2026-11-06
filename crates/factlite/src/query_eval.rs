//! Correctness-first conjunctive evaluation over a replayed historical fact set.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashSet},
};

use crate::{Comparison, Fact, Params, Query, QueryName, QueryResult, Result, Schema, Term, Value};

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

/// Every extension of `bindings` that matches all `patterns`.
fn extend(
    mut bindings: Vec<Binding>,
    patterns: &[crate::Pattern],
    facts: &HashSet<Fact>,
    params: &Params,
) -> Vec<Binding> {
    for pattern in patterns {
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
            break;
        }
    }
    bindings
}

fn passes(binding: &Binding, predicates: &[crate::Predicate], params: &Params) -> Result<bool> {
    for predicate in predicates {
        if !compare(
            resolve(&predicate.left, binding, params),
            predicate.comparison,
            resolve(&predicate.right, binding, params),
        )? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Whether some negated group holds for `binding`: an extension of its join
/// variables (all of them for `not`) satisfies the group's patterns and
/// predicates.
fn negated(
    binding: &Binding,
    query: &Query,
    facts: &HashSet<Fact>,
    params: &Params,
) -> Result<bool> {
    for negation in &query.negations {
        let start = match &negation.join {
            None => binding.clone(),
            Some(join) => join
                .iter()
                .map(|name| {
                    (
                        name.clone(),
                        binding.get(name).expect("validated join").clone(),
                    )
                })
                .collect(),
        };
        for extension in extend(vec![start], &negation.patterns, facts, params) {
            if passes(&extension, &negation.predicates, params)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub(crate) fn evaluate(
    query: &Query,
    params: &Params,
    schema: &Schema,
    facts: &HashSet<Fact>,
) -> Result<QueryResult> {
    query.validate(schema, params)?;
    let empty = || QueryResult {
        columns: crate::query_aggregate::result_columns(query),
        rows: Vec::new(),
    };
    if query.limit == Some(0) {
        return Ok(empty());
    }
    let bindings = extend(vec![Binding::new()], &query.patterns, facts, params);
    if bindings.is_empty() {
        return Ok(empty());
    }
    // Aggregates project their tuple columns; plain queries project selections.
    let projected = if query.aggregates.is_empty() {
        query.select.clone()
    } else {
        crate::query_aggregate::tuple_columns(query)
    };
    let mut distinct = HashSet::new();
    for binding in bindings {
        if passes(&binding, &query.predicates, params)? && !negated(&binding, query, facts, params)?
        {
            distinct.insert(
                projected
                    .iter()
                    .map(|name| binding.get(name).expect("validated selection").clone())
                    .collect::<Vec<_>>(),
            );
        }
    }
    if !query.aggregates.is_empty() {
        return crate::query_aggregate::finish(query, distinct.into_iter().collect());
    }
    let mut rows: Vec<_> = distinct.into_iter().collect();
    crate::query_aggregate::order_and_limit(query, &query.select, &mut rows);
    Ok(QueryResult {
        columns: query.select.clone(),
        rows,
    })
}
