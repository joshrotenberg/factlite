//! Bounded conjunctive query structure and historical type checking.

use std::collections::{BTreeMap, BTreeSet};

use crate::{AttrName, Db, Error, ErrorCode, ErrorContext, Result, Schema, Value, ValueType};

/// Exact query-local variable/parameter/column name. Names do not become SQL text.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct QueryName(String);
impl QueryName {
    /// Accept an exact, nonempty name without NUL. Unicode is not normalized.
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if name.is_empty() || name.contains('\0') {
            return Err(Error::invalid(
                "query name",
                "query names must be nonempty without NUL",
            ));
        }
        Ok(Self(name))
    }
    /// The caller's exact name, not a generated SQL alias.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Typed position in a pattern or comparison.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Term {
    /// Variable bound by a fact pattern.
    Var(QueryName),
    /// Concrete typed value; entity positions require Ref.
    Const(Value),
    /// Declared typed input.
    Param(QueryName),
}

/// A conjunctive fact pattern with a fixed, historically resolved attribute.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pattern {
    /// Entity domain; constants/parameters must be typed references.
    pub entity: Term,
    /// Fixed qualified attribute, never a variable or SQL expression.
    pub attribute: AttrName,
    /// Exact value domain from this attribute's historical definition.
    pub value: Term,
}

/// Typed equality and same-type range predicates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Comparison {
    /// Exact typed equality; different types compare unequal.
    Eq,
    /// Negation of exact typed equality.
    Ne,
    /// Strict same-type ascending comparison.
    Lt,
    /// Inclusive same-type ascending comparison.
    Le,
    /// Strict same-type descending comparison.
    Gt,
    /// Inclusive same-type descending comparison.
    Ge,
}

/// One comparison, evaluated after matching and before projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Predicate {
    /// Bound variable, typed constant, or declared parameter.
    pub left: Term,
    /// Comparison semantics.
    pub comparison: Comparison,
    /// Bound variable, typed constant, or declared parameter.
    pub right: Term,
}

/// Ordering direction within a selected field's type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    /// Same-type ascending order.
    Asc,
    /// Same-type descending order.
    Desc,
}

/// Ordering may reference only selected columns. Supply a total tie-breaker for
/// predictable limits; non-unique sort keys do not define stable page boundaries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Order {
    /// Selected field name.
    pub field: QueryName,
    /// Sort direction.
    pub direction: Direction,
}

/// Concrete input values, validated against Query's declared input types.
pub type Params = BTreeMap<QueryName, Value>;

/// Conjunctive match/filter/project/distinct/order/limit query.
/// Execution, interchange, and SQL compilation follow this AST without extending
/// it to optional, negative, recursive, or aggregate patterns.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Query {
    /// Declared input names and exact types; every input must be supplied.
    pub inputs: BTreeMap<QueryName, ValueType>,
    /// Conjunctive fact patterns. Shared variables join exact typed values.
    pub patterns: Vec<Pattern>,
    /// Comparisons over bound variables/constants/parameters.
    pub predicates: Vec<Predicate>,
    /// Bound selected columns, without duplicate names.
    pub select: Vec<QueryName>,
    /// Optional ordering over selected fields only.
    pub order: Vec<Order>,
    /// Limit after distinct and ordering. Zero is valid; negative is unrepresentable.
    pub limit: Option<u64>,
}

/// Owned relation of named columns and typed tuples. Entity-domain columns use
/// Value::Ref; rows are distinct. Without a total order their order is unspecified.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryResult {
    /// Unique selected names in caller-supplied order.
    pub columns: Vec<QueryName>,
    /// Typed rows, with one value per selected column.
    pub rows: Vec<Vec<Value>>,
}

fn mismatch(expected: ValueType, actual: ValueType) -> Error {
    Error::new(
        ErrorCode::TypeMismatch,
        ErrorContext::TypeMismatch { expected, actual },
        "incompatible query position types",
    )
}

impl Query {
    /// Validate bindings, parameters, comparisons, projection and ordering against
    /// the supplied schema. This performs no fact matching or database access.
    pub fn validate(&self, schema: &Schema, params: &Params) -> Result<()> {
        self.types(schema, params).map(|_| ())
    }

    pub(crate) fn types(
        &self,
        schema: &Schema,
        params: &Params,
    ) -> Result<BTreeMap<QueryName, ValueType>> {
        for name in params.keys() {
            if !self.inputs.contains_key(name) {
                return Err(Error::invalid(
                    "params",
                    format!("undeclared input: {}", name.as_str()),
                ));
            }
        }
        for (name, expected) in &self.inputs {
            let actual = params
                .get(name)
                .ok_or_else(|| {
                    Error::invalid("params", format!("missing input: {}", name.as_str()))
                })?
                .value_type();
            if *expected != actual {
                return Err(mismatch(*expected, actual));
            }
        }
        let mut variables = BTreeMap::new();
        for pattern in &self.patterns {
            let definition = schema.get(&pattern.attribute).ok_or_else(|| {
                Error::new(
                    ErrorCode::UnknownAttribute,
                    ErrorContext::Attribute {
                        attribute: pattern.attribute.clone(),
                    },
                    "query attribute is not defined at this snapshot",
                )
            })?;
            for (term, expected) in [
                (&pattern.entity, ValueType::Ref),
                (&pattern.value, definition.value_type()),
            ] {
                match term {
                    Term::Var(name) => {
                        if let Some(actual) = variables.insert(name.clone(), expected)
                            && actual != expected
                        {
                            return Err(mismatch(expected, actual));
                        }
                    }
                    _ => {
                        let actual = self.term_type(term, &variables)?;
                        if expected != actual {
                            return Err(mismatch(expected, actual));
                        }
                    }
                }
            }
        }
        for predicate in &self.predicates {
            let left = self.term_type(&predicate.left, &variables)?;
            let right = self.term_type(&predicate.right, &variables)?;
            if !matches!(predicate.comparison, Comparison::Eq | Comparison::Ne) && left != right {
                return Err(mismatch(left, right));
            }
        }
        let mut selected = BTreeSet::new();
        for name in &self.select {
            if !variables.contains_key(name) {
                return Err(Error::invalid(
                    "select",
                    format!("unbound column: {}", name.as_str()),
                ));
            }
            if !selected.insert(name) {
                return Err(Error::invalid("select", "duplicate selected column"));
            }
        }
        for order in &self.order {
            if !selected.contains(&order.field) {
                return Err(Error::invalid("order", "order field must be selected"));
            }
        }
        Ok(variables)
    }

    fn term_type(
        &self,
        term: &Term,
        variables: &BTreeMap<QueryName, ValueType>,
    ) -> Result<ValueType> {
        match term {
            Term::Var(name) => variables.get(name).copied().ok_or_else(|| {
                Error::invalid("query", format!("unbound variable: {}", name.as_str()))
            }),
            Term::Const(value) => Ok(value.value_type()),
            Term::Param(name) => self.inputs.get(name).copied().ok_or_else(|| {
                Error::invalid("inputs", format!("undeclared parameter: {}", name.as_str()))
            }),
        }
    }
}

impl Db {
    /// Validate against this view's historical schema and exact typed inputs.
    /// No query execution is performed by this method.
    pub fn validate_query(&self, query: &Query, params: &Params) -> Result<()> {
        self.read_basis(|transaction| {
            query.validate(
                &crate::index::schema_at(transaction, self.snapshot())?,
                params,
            )
        })
    }
}
