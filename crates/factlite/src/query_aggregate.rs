//! Grouping and aggregation over distinct projected tuples. The SQL path and
//! the reference evaluator both produce the tuples and then call `finish`, so
//! arithmetic, grouping and ordering are identical on both.

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
};

use crate::{AggregateFn, Direction, Error, Query, QueryName, QueryResult, Result, Value};

/// Selected variables, then `with` variables, then aggregate variables, each
/// once: the columns of the distinct tuples that are aggregated.
pub(crate) fn tuple_columns(query: &Query) -> Vec<QueryName> {
    let mut columns = query.select.clone();
    for name in query
        .with
        .iter()
        .chain(query.aggregates.iter().map(|aggregate| &aggregate.var))
    {
        if !columns.contains(name) {
            columns.push(name.clone());
        }
    }
    columns
}

/// The plain query whose distinct rows are the tuples to aggregate.
pub(crate) fn tuple_query(query: &Query) -> Query {
    Query {
        select: tuple_columns(query),
        order: Vec::new(),
        limit: None,
        with: Vec::new(),
        aggregates: Vec::new(),
        ..query.clone()
    }
}

/// The result columns: selected variables, then aggregate names.
pub(crate) fn result_columns(query: &Query) -> Vec<QueryName> {
    query
        .select
        .iter()
        .cloned()
        .chain(
            query
                .aggregates
                .iter()
                .map(|aggregate| aggregate.name.clone()),
        )
        .collect()
}

fn overflow(message: &str) -> Error {
    Error::invalid("aggregates", message)
}

fn aggregate(function: AggregateFn, values: Vec<&Value>) -> Result<Value> {
    let count = |n: usize| {
        i64::try_from(n)
            .map(Value::I64)
            .map_err(|_| overflow("count exceeds i64"))
    };
    match function {
        AggregateFn::Count => count(values.len()),
        AggregateFn::CountDistinct => count(values.into_iter().collect::<HashSet<_>>().len()),
        AggregateFn::Sum => {
            let mut values = values;
            // Ascending order makes the F64 rounding sequence deterministic.
            values.sort_by(|a, b| a.canonical_cmp(b));
            match values.first() {
                Some(Value::I64(_)) => {
                    let mut total: i64 = 0;
                    for value in values {
                        let Value::I64(n) = value else {
                            unreachable!("validated i64 column")
                        };
                        total = total
                            .checked_add(*n)
                            .ok_or_else(|| overflow("i64 sum overflows"))?;
                    }
                    Ok(Value::I64(total))
                }
                Some(Value::F64(_)) => {
                    let mut total = 0.0_f64;
                    for value in values {
                        let Value::F64(x) = value else {
                            unreachable!("validated f64 column")
                        };
                        total += x.get();
                        if !total.is_finite() {
                            return Err(overflow("f64 sum is not finite"));
                        }
                    }
                    Value::f64(total)
                }
                _ => unreachable!("validated numeric, nonempty group"),
            }
        }
        AggregateFn::Min | AggregateFn::Max => {
            let mut values = values.into_iter();
            let mut best = values.next().expect("groups are nonempty").clone();
            for value in values {
                let ordering = value.compare(&best).expect("validated uniform column");
                let better = match function {
                    AggregateFn::Min => ordering == Ordering::Less,
                    _ => ordering == Ordering::Greater,
                };
                if better {
                    best = value.clone();
                }
            }
            Ok(best)
        }
    }
}

/// Orders rows by the query's order fields among `columns`, then applies the
/// limit. Rows without a total order keep an unspecified order.
pub(crate) fn order_and_limit(query: &Query, columns: &[QueryName], rows: &mut Vec<Vec<Value>>) {
    let order: Vec<_> = query
        .order
        .iter()
        .map(|order| {
            (
                columns
                    .iter()
                    .position(|name| name == &order.field)
                    .expect("validated order field"),
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
}

/// Groups distinct `tuples` (in `tuple_columns` order) by the selected
/// variables, aggregates each group, then orders and limits.
pub(crate) fn finish(query: &Query, tuples: Vec<Vec<Value>>) -> Result<QueryResult> {
    let columns = tuple_columns(query);
    let groups_len = query.select.len();
    let mut groups: HashMap<Vec<Value>, Vec<Vec<Value>>> = HashMap::new();
    for tuple in tuples {
        groups
            .entry(tuple[..groups_len].to_vec())
            .or_default()
            .push(tuple);
    }
    let positions: Vec<_> = query
        .aggregates
        .iter()
        .map(|aggregate| {
            columns
                .iter()
                .position(|name| name == &aggregate.var)
                .expect("aggregate variable is a tuple column")
        })
        .collect();
    let mut rows = Vec::with_capacity(groups.len());
    for (mut key, members) in groups {
        for (aggregate, position) in query.aggregates.iter().zip(&positions) {
            key.push(self::aggregate(
                aggregate.function,
                members.iter().map(|tuple| &tuple[*position]).collect(),
            )?);
        }
        rows.push(key);
    }
    let result_columns = result_columns(query);
    order_and_limit(query, &result_columns, &mut rows);
    Ok(QueryResult {
        columns: result_columns,
        rows,
    })
}
