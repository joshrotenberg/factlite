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

/// An aggregate function over one variable within each group.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AggregateFn {
    /// The number of distinct projected tuples in the group, as I64.
    Count,
    /// The number of distinct values of the variable in the group, as I64.
    CountDistinct,
    /// The sum over the group's tuples of an I64 or F64 variable, in its type.
    /// I64 overflow is an error. F64 values are added in ascending order, and a
    /// non-finite running total is an error.
    Sum,
    /// The least value of the variable in the group, in its type.
    Min,
    /// The greatest value of the variable in the group, in its type.
    Max,
}

/// One aggregate column, named in the result after the selected columns.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Aggregate {
    /// The function applied within each group.
    pub function: AggregateFn,
    /// A variable bound by the patterns.
    pub var: QueryName,
    /// The result column name; distinct from every other column.
    pub name: QueryName,
}

/// A negated group: a binding survives only if no extension of its join
/// variables satisfies every pattern and predicate in the group at the view's
/// revision. Groups do not nest.
///
/// With `join` absent (`not`), every variable in the group must be bound by the
/// query's positive patterns. With `join` present (`not-join`), the named
/// variables must be bound by the positive patterns; every other variable in
/// the group is local to it, must be bound by a pattern in the group, and must
/// not share a name with a variable bound outside.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Negation {
    /// `None` for `not`; the join variables for `not-join`.
    pub join: Option<Vec<QueryName>>,
    /// Fact patterns that must all match for the group to hold.
    pub patterns: Vec<Pattern>,
    /// Comparisons over the group's variables, constants and parameters.
    pub predicates: Vec<Predicate>,
}

/// Concrete input values, validated against Query's declared input types.
pub type Params = BTreeMap<QueryName, Value>;

/// Conjunctive match/filter/project/distinct/order/limit query.
/// Execution, interchange, and SQL compilation follow this AST. Negation is
/// limited to one level of `not` and `not-join` groups; there are no optional,
/// disjunctive or recursive patterns.
///
/// With aggregates, evaluation is: match, filter and apply negations; project the selected
/// variables, the `with` variables and each aggregate's variable; remove
/// duplicate tuples; group by the selected variables; aggregate each group;
/// then order and limit. Only distinct tuples are aggregated, so include a
/// variable in `with` (such as the entity) when equal values from different
/// matches must each count. No matching tuples yield no rows, even without
/// selected variables.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Query {
    /// Declared input names and exact types; every input must be supplied.
    pub inputs: BTreeMap<QueryName, ValueType>,
    /// Conjunctive fact patterns. Shared variables join exact typed values.
    pub patterns: Vec<Pattern>,
    /// Comparisons over bound variables/constants/parameters.
    pub predicates: Vec<Predicate>,
    /// Negated groups, applied with the predicates before projection.
    pub negations: Vec<Negation>,
    /// Bound selected columns, without duplicate names.
    pub select: Vec<QueryName>,
    /// Optional ordering over selected fields and aggregate names only.
    pub order: Vec<Order>,
    /// Limit after distinct and ordering. Zero is valid; negative is unrepresentable.
    pub limit: Option<u64>,
    /// Bound variables kept distinct in aggregated tuples but not returned.
    /// Requires at least one aggregate.
    pub with: Vec<QueryName>,
    /// Aggregate columns, after the selected columns. Selected variables group.
    pub aggregates: Vec<Aggregate>,
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
            self.bind_pattern(pattern, schema, &mut variables, Scope::Open)?;
        }
        for predicate in &self.predicates {
            let left = self.term_type(&predicate.left, &variables)?;
            let right = self.term_type(&predicate.right, &variables)?;
            if !matches!(predicate.comparison, Comparison::Eq | Comparison::Ne) && left != right {
                return Err(mismatch(left, right));
            }
        }
        for negation in &self.negations {
            self.negation_scope(negation, schema, &variables)?;
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
        if !self.with.is_empty() && self.aggregates.is_empty() {
            return Err(Error::invalid(
                "with",
                "with requires at least one aggregate",
            ));
        }
        let mut kept = BTreeSet::new();
        for name in &self.with {
            if !variables.contains_key(name) {
                return Err(Error::invalid(
                    "with",
                    format!("unbound variable: {}", name.as_str()),
                ));
            }
            if selected.contains(name) || !kept.insert(name) {
                return Err(Error::invalid(
                    "with",
                    "with variables must be distinct and not selected",
                ));
            }
        }
        for aggregate in &self.aggregates {
            let ty = *variables.get(&aggregate.var).ok_or_else(|| {
                Error::invalid(
                    "aggregates",
                    format!("unbound variable: {}", aggregate.var.as_str()),
                )
            })?;
            if aggregate.function == AggregateFn::Sum
                && !matches!(ty, ValueType::I64 | ValueType::F64)
            {
                return Err(Error::new(
                    ErrorCode::TypeMismatch,
                    ErrorContext::TypeMismatch {
                        expected: ValueType::I64,
                        actual: ty,
                    },
                    "sum requires an i64 or f64 variable",
                ));
            }
            if !selected.insert(&aggregate.name) {
                return Err(Error::invalid(
                    "aggregates",
                    "aggregate name duplicates another column",
                ));
            }
        }
        for order in &self.order {
            if !selected.contains(&order.field) {
                return Err(Error::invalid(
                    "order",
                    "order field must be selected or an aggregate",
                ));
            }
        }
        Ok(variables)
    }

    /// Type-checks one pattern, binding new variables as `scope` allows.
    fn bind_pattern(
        &self,
        pattern: &Pattern,
        schema: &Schema,
        variables: &mut BTreeMap<QueryName, ValueType>,
        scope: Scope<'_>,
    ) -> Result<()> {
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
                    if let Some(actual) = variables.get(name) {
                        if *actual != expected {
                            return Err(mismatch(expected, *actual));
                        }
                        continue;
                    }
                    match scope {
                        Scope::Open => {}
                        Scope::Closed => {
                            return Err(Error::invalid(
                                "not",
                                format!(
                                    "not variables must be bound outside the group: {}",
                                    name.as_str()
                                ),
                            ));
                        }
                        Scope::Local(outer) => {
                            if outer.contains_key(name) {
                                return Err(Error::invalid(
                                    "not-join",
                                    format!(
                                        "local variable shadows an outer variable: {}",
                                        name.as_str()
                                    ),
                                ));
                            }
                        }
                    }
                    variables.insert(name.clone(), expected);
                }
                _ => {
                    let actual = self.term_type(term, variables)?;
                    if expected != actual {
                        return Err(mismatch(expected, actual));
                    }
                }
            }
        }
        Ok(())
    }

    /// Validates a negated group against the outer variables and returns the
    /// variables visible inside it, with their types: join (or all outer)
    /// variables, then group locals.
    pub(crate) fn negation_scope(
        &self,
        negation: &Negation,
        schema: &Schema,
        outer: &BTreeMap<QueryName, ValueType>,
    ) -> Result<BTreeMap<QueryName, ValueType>> {
        if negation.patterns.is_empty() && negation.predicates.is_empty() {
            return Err(Error::invalid(
                "not",
                "a negated group needs at least one pattern or predicate",
            ));
        }
        let (mut scope, rule) = match &negation.join {
            None => (outer.clone(), Scope::Closed),
            Some(join) => {
                let mut scope = BTreeMap::new();
                for name in join {
                    let ty = *outer.get(name).ok_or_else(|| {
                        Error::invalid(
                            "not-join",
                            format!(
                                "join variables must be bound outside the group: {}",
                                name.as_str()
                            ),
                        )
                    })?;
                    if scope.insert(name.clone(), ty).is_some() {
                        return Err(Error::invalid("not-join", "duplicate join variable"));
                    }
                }
                (scope, Scope::Local(outer))
            }
        };
        for pattern in &negation.patterns {
            self.bind_pattern(pattern, schema, &mut scope, rule)?;
        }
        for predicate in &negation.predicates {
            let left = self.group_term_type(&predicate.left, &scope, outer, rule)?;
            let right = self.group_term_type(&predicate.right, &scope, outer, rule)?;
            if !matches!(predicate.comparison, Comparison::Eq | Comparison::Ne) && left != right {
                return Err(mismatch(left, right));
            }
        }
        Ok(scope)
    }

    fn group_term_type(
        &self,
        term: &Term,
        scope: &BTreeMap<QueryName, ValueType>,
        outer: &BTreeMap<QueryName, ValueType>,
        rule: Scope<'_>,
    ) -> Result<ValueType> {
        if let Term::Var(name) = term
            && !scope.contains_key(name)
        {
            return Err(match rule {
                Scope::Local(_) if outer.contains_key(name) => Error::invalid(
                    "not-join",
                    format!("outer variable is not a join variable: {}", name.as_str()),
                ),
                Scope::Local(_) => Error::invalid(
                    "not-join",
                    format!(
                        "local variable must be bound by a pattern in the group: {}",
                        name.as_str()
                    ),
                ),
                _ => Error::invalid(
                    "not",
                    format!(
                        "not variables must be bound outside the group: {}",
                        name.as_str()
                    ),
                ),
            });
        }
        self.term_type(term, scope)
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

/// Whether a pattern may bind variables not already in scope.
#[derive(Clone, Copy)]
enum Scope<'a> {
    /// Positive patterns: any new variable is bound.
    Open,
    /// `not`: no new variables.
    Closed,
    /// `not-join`: new variables are local, and must not name an outer one.
    Local(&'a BTreeMap<QueryName, ValueType>),
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
