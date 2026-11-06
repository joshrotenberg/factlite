//! Grouping and aggregation over distinct projected tuples. The SQL path and
//! the reference evaluator both produce the tuples and then call `finish`, so
//! arithmetic, grouping and ordering are identical on both.

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
};

use crate::{
    AggregateFn, Direction, Error, Params, Query, QueryName, QueryResult, Result, Term, Value,
};

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
        having: Vec::new(),
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

/// Adds F64 values in ascending order, so the rounding sequence is
/// deterministic. A non-finite running total is an error.
fn f64_sum(values: &[&Value]) -> Result<f64> {
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
    Ok(total)
}

/// The f64 nearest to `numerator / denominator`, ties to even, rounded once.
/// Requires `0 < numerator < 2^127` and `0 < denominator < 2^64`, so the
/// quotient is a normal f64.
fn nearest_quotient(numerator: u128, denominator: u128) -> f64 {
    debug_assert!(numerator > 0 && numerator < 1 << 127);
    debug_assert!(denominator > 0 && denominator < 1 << 64);
    let bits = |n: u128| i32::try_from(128 - n.leading_zeros()).expect("at most 128");
    // Scale by 2^k so the integer quotient has exactly 53 significant bits.
    let divide = |k: i32| {
        let (n, d) = if k >= 0 {
            (numerator << k, denominator)
        } else {
            (numerator, denominator << -k)
        };
        (n / d, n % d, d)
    };
    let mut k = 52 - bits(numerator) + bits(denominator);
    let (mut quotient, mut remainder, mut divisor) = divide(k);
    if quotient < 1 << 52 {
        k += 1;
        (quotient, remainder, divisor) = divide(k);
    }
    debug_assert!((1 << 52..1 << 53).contains(&quotient));
    if 2 * remainder > divisor || (2 * remainder == divisor && quotient & 1 == 1) {
        quotient += 1;
    }
    let scale = f64::from_bits(u64::try_from(1023 - k).expect("normal exponent") << 52);
    // `quotient` is at most 2^53 and the scale a power of two, so both
    // conversions and the product are exact.
    quotient as f64 * scale
}

/// The mean of nonempty I64 values: the exact sum over the count, rounded once.
fn i64_mean(values: &[&Value]) -> f64 {
    let total: i128 = values
        .iter()
        .map(|value| match value {
            Value::I64(n) => i128::from(*n),
            _ => unreachable!("validated i64 column"),
        })
        .sum();
    if total == 0 {
        return 0.0;
    }
    let count = u128::try_from(values.len()).expect("usize fits u128");
    let mean = nearest_quotient(total.unsigned_abs(), count);
    if total < 0 { -mean } else { mean }
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
                Some(Value::F64(_)) => Value::f64(f64_sum(&values)?),
                _ => unreachable!("validated numeric, nonempty group"),
            }
        }
        AggregateFn::Avg => {
            let mut values = values;
            values.sort_by(|a, b| a.canonical_cmp(b));
            match values.first() {
                Some(Value::I64(_)) => Value::f64(i64_mean(&values)),
                Some(Value::F64(_)) => Value::f64(f64_sum(&values)? / values.len() as f64),
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
/// variables, aggregates every group, keeps the rows that pass `having`, then
/// orders and limits. An aggregate error in any group fails the query, even a
/// group that `having` would drop.
pub(crate) fn finish(
    query: &Query,
    params: &Params,
    tuples: Vec<Vec<Value>>,
) -> Result<QueryResult> {
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
    if !query.having.is_empty() {
        let resolve = |term: &'_ Term, row: &'_ [Value]| -> Value {
            match term {
                Term::Var(name) => row[result_columns
                    .iter()
                    .position(|column| column == name)
                    .expect("validated having column")]
                .clone(),
                Term::Const(value) => value.clone(),
                Term::Param(name) => params.get(name).expect("validated input").clone(),
            }
        };
        let mut kept = Vec::with_capacity(rows.len());
        for row in rows {
            let mut passes = true;
            for predicate in &query.having {
                if !crate::query_eval::compare(
                    &resolve(&predicate.left, &row),
                    predicate.comparison,
                    &resolve(&predicate.right, &row),
                )? {
                    passes = false;
                    break;
                }
            }
            if passes {
                kept.push(row);
            }
        }
        rows = kept;
    }
    order_and_limit(query, &result_columns, &mut rows);
    Ok(QueryResult {
        columns: result_columns,
        rows,
    })
}
