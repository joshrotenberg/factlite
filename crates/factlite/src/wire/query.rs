//! Strict query envelopes using the same tagged values as transaction input.

use serde_json::{Map, Value as Json, json};

use super::{Object, concrete_value, strict, text, value_json};
use crate::{
    Aggregate, AggregateFn, Comparison, Direction, Error, Order, Params, Pattern, Predicate, Query,
    QueryName, QueryResult, Result, Term, ValueType,
};

fn term(value: Json) -> Result<Term> {
    if let Json::Object(fields) = &value
        && (fields.contains_key("var") || fields.contains_key("param"))
    {
        let mut object = Object::new(value, "term")?;
        let term = match (object.optional("var"), object.optional("param")) {
            (Some(value), None) => Term::Var(QueryName::new(text(value, "var")?)?),
            (None, Some(value)) => Term::Param(QueryName::new(text(value, "param")?)?),
            _ => {
                return Err(Error::invalid(
                    "term",
                    "expected exactly one var or param tag",
                ));
            }
        };
        object.finish()?;
        return Ok(term);
    }
    Ok(Term::Const(concrete_value(value)?))
}
fn term_json(term: &Term) -> Json {
    match term {
        Term::Var(name) => json!({"var":name.as_str()}),
        Term::Param(name) => json!({"param":name.as_str()}),
        Term::Const(value) => value_json(value),
    }
}
fn ty(value: Json) -> Result<ValueType> {
    super::type_from_name(&text(value, "inputs")?)
        .ok_or_else(|| Error::invalid("inputs", "unknown input type"))
}
fn ty_json(ty: ValueType) -> Json {
    json!(super::type_name(ty))
}
fn comparison(value: Json) -> Result<Comparison> {
    match text(value, "op")?.as_str() {
        "eq" => Ok(Comparison::Eq),
        "ne" => Ok(Comparison::Ne),
        "lt" => Ok(Comparison::Lt),
        "le" => Ok(Comparison::Le),
        "gt" => Ok(Comparison::Gt),
        "ge" => Ok(Comparison::Ge),
        _ => Err(Error::invalid("op", "unknown query comparison")),
    }
}
fn comparison_json(comparison: Comparison) -> Json {
    json!(match comparison {
        Comparison::Eq => "eq",
        Comparison::Ne => "ne",
        Comparison::Lt => "lt",
        Comparison::Le => "le",
        Comparison::Gt => "gt",
        Comparison::Ge => "ge",
    })
}
fn fields(value: Option<Json>, field: &'static str) -> Result<Map<String, Json>> {
    match value {
        None => Ok(Map::new()),
        Some(Json::Object(fields)) => Ok(fields),
        _ => Err(Error::invalid(field, "expected an object")),
    }
}
fn required_array(object: &mut Object, key: &str) -> Result<Vec<Json>> {
    match object.required(key)? {
        Json::Array(values) => Ok(values),
        _ => Err(Error::invalid("query", format!("expected array: {key}"))),
    }
}

/// Decode a version-one query/params envelope. Match and select are required;
/// inputs/where/order/params default empty and limit defaults absent. Historical
/// schema and variable-domain validation runs through Db::query/validate_query.
pub fn decode_query(input: &str) -> Result<(Query, Params)> {
    let mut envelope = Object::new(strict::parse(input)?, "query envelope")?;
    envelope.version()?;
    let mut object = Object::new(envelope.required("query")?, "query")?;
    let inputs = fields(object.optional("inputs"), "inputs")?
        .into_iter()
        .map(|(name, value)| Ok((QueryName::new(name)?, ty(value)?)))
        .collect::<Result<_>>()?;
    let patterns = required_array(&mut object, "match")?
        .into_iter()
        .map(|value| {
            let Json::Array(values) = value else {
                return Err(Error::invalid(
                    "match",
                    "expected three-position fact pattern",
                ));
            };
            let [entity, attribute, value]: [Json; 3] = values
                .try_into()
                .map_err(|_| Error::invalid("match", "expected exactly three positions"))?;
            Ok(Pattern {
                entity: term(entity)?,
                attribute: text(attribute, "attribute")?.parse()?,
                value: term(value)?,
            })
        })
        .collect::<Result<_>>()?;
    let predicates = object
        .array("where")?
        .into_iter()
        .map(|value| {
            let mut object = Object::new(value, "predicate")?;
            let left = term(object.required("left")?)?;
            let comparison = comparison(object.required("op")?)?;
            let right = term(object.required("right")?)?;
            object.finish()?;
            Ok(Predicate {
                left,
                comparison,
                right,
            })
        })
        .collect::<Result<_>>()?;
    let select = required_array(&mut object, "select")?
        .into_iter()
        .map(|value| QueryName::new(text(value, "select")?))
        .collect::<Result<_>>()?;
    let order = object
        .array("order")?
        .into_iter()
        .map(|value| {
            let mut object = Object::new(value, "order")?;
            let field = QueryName::new(text(object.required("field")?, "field")?)?;
            let direction = match text(object.required("direction")?, "direction")?.as_str() {
                "asc" => Direction::Asc,
                "desc" => Direction::Desc,
                _ => return Err(Error::invalid("direction", "unknown order direction")),
            };
            object.finish()?;
            Ok(Order { field, direction })
        })
        .collect::<Result<_>>()?;
    let with = object
        .array("with")?
        .into_iter()
        .map(|value| QueryName::new(text(value, "with")?))
        .collect::<Result<_>>()?;
    let aggregates = object
        .array("aggregates")?
        .into_iter()
        .map(|value| {
            let mut object = Object::new(value, "aggregate")?;
            let function = match text(object.required("fn")?, "fn")?.as_str() {
                "count" => AggregateFn::Count,
                "count_distinct" => AggregateFn::CountDistinct,
                "sum" => AggregateFn::Sum,
                "min" => AggregateFn::Min,
                "max" => AggregateFn::Max,
                _ => return Err(Error::invalid("fn", "unknown aggregate function")),
            };
            let var = QueryName::new(text(object.required("var")?, "var")?)?;
            let name = QueryName::new(text(object.required("name")?, "name")?)?;
            object.finish()?;
            Ok(Aggregate {
                function,
                var,
                name,
            })
        })
        .collect::<Result<_>>()?;
    let limit = object
        .optional("limit")
        .map(|value| {
            value
                .as_u64()
                .ok_or_else(|| Error::invalid("limit", "limit must be a nonnegative integer"))
        })
        .transpose()?;
    object.finish()?;
    let params = fields(envelope.optional("params"), "params")?
        .into_iter()
        .map(|(name, value)| Ok((QueryName::new(name)?, concrete_value(value)?)))
        .collect::<Result<_>>()?;
    envelope.finish()?;
    Ok((
        Query {
            inputs,
            patterns,
            predicates,
            select,
            order,
            limit,
            with,
            aggregates,
        },
        params,
    ))
}

/// Encode a query/params envelope, preserving pattern, predicate, selection and
/// ordering arrays. Values share the lossless transaction tagging contract.
pub fn encode_query(query: &Query, params: &Params) -> String {
    let mut inner = json!({
        "inputs":query.inputs.iter().map(|(name,ty)| (name.as_str().to_owned(),ty_json(*ty))).collect::<Map<_,_>>(),
        "match":query.patterns.iter().map(|pattern| json!([term_json(&pattern.entity),pattern.attribute.as_str(),term_json(&pattern.value)])).collect::<Vec<_>>(),
        "where":query.predicates.iter().map(|predicate| json!({"left":term_json(&predicate.left),"op":comparison_json(predicate.comparison),"right":term_json(&predicate.right)})).collect::<Vec<_>>(),
        "select":query.select.iter().map(QueryName::as_str).collect::<Vec<_>>(),
        "order":query.order.iter().map(|order| json!({"field":order.field.as_str(),"direction":match order.direction { Direction::Asc => "asc",Direction::Desc => "desc" }})).collect::<Vec<_>>()
    });
    if let Some(limit) = query.limit {
        inner["limit"] = json!(limit);
    }
    if !query.with.is_empty() {
        inner["with"] = json!(query.with.iter().map(QueryName::as_str).collect::<Vec<_>>());
    }
    if !query.aggregates.is_empty() {
        inner["aggregates"] = json!(
            query
                .aggregates
                .iter()
                .map(|aggregate| json!({
                    "fn": match aggregate.function {
                        AggregateFn::Count => "count",
                        AggregateFn::CountDistinct => "count_distinct",
                        AggregateFn::Sum => "sum",
                        AggregateFn::Min => "min",
                        AggregateFn::Max => "max",
                    },
                    "var": aggregate.var.as_str(),
                    "name": aggregate.name.as_str(),
                }))
                .collect::<Vec<_>>()
        );
    }
    json!({"version":1,"query":inner,"params":params.iter().map(|(name,value)| (name.as_str().to_owned(),value_json(value))).collect::<Map<_,_>>()}).to_string()
}

/// Encode named typed rows, including typed references for entity columns.
pub fn encode_query_result(result: &QueryResult) -> String {
    json!({"version":1,"columns":result.columns.iter().map(QueryName::as_str).collect::<Vec<_>>(),"rows":result.rows.iter().map(|row| row.iter().map(value_json).collect::<Vec<_>>()).collect::<Vec<_>>()}).to_string()
}
