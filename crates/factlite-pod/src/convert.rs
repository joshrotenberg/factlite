//! Translation between Clojure data (as transit values) and Factlite.
//!
//! Inputs become version-1 wire JSON and go through the library's strict wire
//! decoders, so the pod adds no second validator. Each Factlite value type has
//! exactly one Clojure representation, in both directions:
//!
//! | Factlite | Clojure |
//! | --- | --- |
//! | `Bool` | boolean |
//! | `I64` | long |
//! | `F64` | double (finite) |
//! | `String` | string |
//! | `Bytes` | byte array |
//! | `Ref` | `{:id "e1..."}` (`{:temp "label"}` in transaction changes) |
//!
//! Anything else in a value position (nil, keywords, symbols, big or ratio
//! numbers, collections, insts, uuids) is rejected, never coerced.

use babashka_pod::PodError;
use babashka_pod::transit::Value as Clj;
use base64::Engine;
use factlite::{
    AttrName, AttributeDefinition, Cardinality, Entity, Fact, FactDelta, FactEvent, FactFilter,
    Operation, PulledValue, QueryResult, SnapshotId, TxReport, Uniqueness, Value, ValueType, wire,
};
use serde_json::{Map, Value as Json, json};

pub(crate) type Result<T> = std::result::Result<T, PodError>;

// ---------------------------------------------------------------------------
// Errors

pub(crate) fn kw(name: &str) -> Clj {
    Clj::Keyword(name.to_owned())
}

fn string(text: &str) -> Clj {
    Clj::String(text.to_owned())
}

fn map(entries: Vec<(&str, Clj)>) -> Clj {
    Clj::Map(entries.into_iter().map(|(k, v)| (kw(k), v)).collect())
}

fn error_data(code: &str, message: String, context: Clj, outcome: Option<&str>) -> PodError {
    let mut entries = vec![
        ("code", string(code)),
        ("message", string(&message)),
        ("context", context),
    ];
    if let Some(outcome) = outcome {
        entries.push(("commit-outcome", string(outcome)));
    }
    let data = serde_json::to_value(map(entries)).unwrap_or(Json::Null);
    PodError::new(message).with_data(data)
}

/// A pod-level argument error, in the same shape as a library `InvalidRequest`.
pub(crate) fn invalid(field: &str, message: impl Into<String>) -> PodError {
    error_data(
        "InvalidRequest",
        message.into(),
        map(vec![("field", string(field))]),
        None,
    )
}

/// A library error as `ex-info`: message, and `ex-data` holding the stable
/// code, structured context and any commit outcome from the wire encoding.
pub(crate) fn library_error(error: factlite::Error) -> PodError {
    let encoded: Json = serde_json::from_str(&wire::encode_error(&error)).unwrap_or(Json::Null);
    let error = &encoded["error"];
    error_data(
        error["code"].as_str().unwrap_or("Storage"),
        error["message"].as_str().unwrap_or_default().to_owned(),
        context_out(&error["context"]),
        error["commit_outcome"].as_str(),
    )
}

fn kebab(key: &str) -> String {
    key.replace('_', "-")
}

// Error contexts have fixed key names; values are converted by key.
fn context_out(context: &Json) -> Clj {
    let Json::Object(fields) = context else {
        return Clj::Nil;
    };
    Clj::Map(
        fields
            .iter()
            .map(|(key, value)| {
                let value = match (key.as_str(), value) {
                    ("attr", Json::String(name)) => kw(name),
                    ("entity", Json::Object(entity)) => entity
                        .get("id")
                        .and_then(Json::as_str)
                        .map_or(Clj::Nil, ref_id),
                    ("value", value) => wire_value_out(value),
                    ("owners", Json::Array(owners)) => Clj::Vector(
                        owners
                            .iter()
                            .map(|id| id.as_str().map_or(Clj::Nil, ref_id))
                            .collect(),
                    ),
                    ("expected" | "actual", Json::Array(values)) => {
                        Clj::Vector(values.iter().map(wire_value_out).collect())
                    }
                    ("expected" | "actual", Json::Object(_)) => {
                        wire::decode_snapshot(&value.to_string())
                            .map_or(Clj::Nil, |snapshot| snapshot_out(&snapshot))
                    }
                    ("expected" | "actual", Json::String(ty)) => kw(ty),
                    (_, Json::String(text)) => string(text),
                    (_, other) => string(&other.to_string()),
                };
                (kw(&kebab(key)), value)
            })
            .collect(),
    )
}

fn wire_value_out(value: &Json) -> Clj {
    wire::decode_value(&value.to_string()).map_or(Clj::Nil, |value| value_out(&value))
}

// ---------------------------------------------------------------------------
// Reading Clojure input

fn unmeta(value: &Clj) -> &Clj {
    match value {
        Clj::WithMeta(inner, _) => unmeta(inner),
        other => other,
    }
}

fn describe(value: &Clj) -> &'static str {
    match value {
        Clj::Nil => "nil",
        Clj::Bool(_) => "a boolean",
        Clj::Int(_) => "a long",
        Clj::BigInt(_) => "a BigInt",
        Clj::Float(_) => "a double",
        Clj::BigDec(_) => "a BigDecimal",
        Clj::Ratio { .. } => "a ratio",
        Clj::String(_) => "a string",
        Clj::Char(_) => "a character",
        Clj::Keyword(_) => "a keyword",
        Clj::Symbol(_) => "a symbol",
        Clj::Inst(_) => "an inst",
        Clj::Uuid(_) => "a uuid",
        Clj::Uri(_) => "a URI",
        Clj::Bytes(_) => "a byte array",
        Clj::List(_) => "a list",
        Clj::Vector(_) => "a vector",
        Clj::Set(_) => "a set",
        Clj::Map(_) => "a map",
        Clj::Tagged(..) => "a tagged value",
        Clj::WithMeta(..) => "a value with metadata",
        _ => "an unsupported value",
    }
}

/// A map with keyword keys whose fields are consumed one by one; any key left
/// over at `finish` is an unknown field, as in the wire decoders.
pub(crate) struct Fields {
    entries: Vec<(String, Clj)>,
    what: &'static str,
}

impl Fields {
    pub(crate) fn new(value: &Clj, what: &'static str) -> Result<Self> {
        let Clj::Map(entries) = unmeta(value) else {
            return Err(invalid(
                what,
                format!("{what} must be a map, got {}", describe(value)),
            ));
        };
        let mut fields: Vec<(String, Clj)> = Vec::with_capacity(entries.len());
        for (key, value) in entries {
            let Clj::Keyword(key) = unmeta(key) else {
                return Err(invalid(what, format!("{what} keys must be keywords")));
            };
            if fields.iter().any(|(seen, _)| seen == key) {
                return Err(invalid(what, format!("duplicate {what} key :{key}")));
            }
            fields.push((key.clone(), value.clone()));
        }
        Ok(Self {
            entries: fields,
            what,
        })
    }

    pub(crate) fn optional(value: Option<&Clj>, what: &'static str) -> Result<Self> {
        match value {
            None | Some(Clj::Nil) => Ok(Self {
                entries: Vec::new(),
                what,
            }),
            Some(value) => Self::new(value, what),
        }
    }

    pub(crate) fn take(&mut self, key: &str) -> Option<Clj> {
        let index = self.entries.iter().position(|(seen, _)| seen == key)?;
        Some(self.entries.remove(index).1)
    }

    fn required(&mut self, key: &str) -> Result<Clj> {
        self.take(key)
            .ok_or_else(|| invalid(self.what, format!("{} requires :{key}", self.what)))
    }

    pub(crate) fn finish(self) -> Result<()> {
        match self.entries.first() {
            None => Ok(()),
            Some((key, _)) => Err(invalid(
                self.what,
                format!("unknown {} key :{key}", self.what),
            )),
        }
    }
}

/// A keyword or string, as text: attribute names, labels, metadata keys.
fn name(value: &Clj, field: &str) -> Result<String> {
    match unmeta(value) {
        Clj::Keyword(name) | Clj::String(name) => Ok(name.clone()),
        other => Err(invalid(
            field,
            format!(
                "{field} must be a keyword or string, got {}",
                describe(other)
            ),
        )),
    }
}

pub(crate) fn text(value: &Clj, field: &str) -> Result<String> {
    match unmeta(value) {
        Clj::String(text) => Ok(text.clone()),
        other => Err(invalid(
            field,
            format!("{field} must be a string, got {}", describe(other)),
        )),
    }
}

fn items<'a>(value: &'a Clj, field: &str) -> Result<&'a [Clj]> {
    match unmeta(value) {
        Clj::Vector(items) | Clj::List(items) => Ok(items),
        other => Err(invalid(
            field,
            format!("{field} must be a vector, got {}", describe(other)),
        )),
    }
}

fn exactly<'a, const N: usize>(value: &'a Clj, field: &str, shape: &str) -> Result<&'a [Clj; N]> {
    items(value, field)?
        .try_into()
        .map_err(|_| invalid(field, format!("{field} entries must be {shape}")))
}

/// A Clojure value as a wire-tagged Factlite value.
pub(crate) fn value_in(value: &Clj, field: &str) -> Result<Json> {
    Ok(match unmeta(value) {
        Clj::Bool(value) => json!({"bool": value}),
        Clj::Int(value) => json!({"i64": value.to_string()}),
        Clj::Float(value) if !value.is_finite() => {
            return Err(invalid(field, "doubles must be finite"));
        }
        // Rust's shortest round-trip decimal; the wire decoder normalizes -0.0.
        Clj::Float(value) => json!({"f64": value.to_string()}),
        Clj::String(value) => json!({"string": value}),
        Clj::Bytes(value) => {
            json!({"bytes": base64::engine::general_purpose::STANDARD.encode(value)})
        }
        Clj::Map(_) => json!({"ref": entity_in(value, field)?}),
        other => {
            return Err(invalid(
                field,
                format!(
                    "unsupported {field} {}: Factlite values are booleans, longs, \
                     doubles, strings, byte arrays or {{:id ...}} references",
                    describe(other)
                ),
            ));
        }
    })
}

/// `{:id "e1..."}` or `{:temp label}` as a wire entity term.
fn entity_in(value: &Clj, field: &str) -> Result<Json> {
    let mut fields = Fields::new(value, "entity")?;
    let entity = match (fields.take("id"), fields.take("temp")) {
        (Some(id), None) => json!({"id": text(&id, "id")?}),
        (None, Some(label)) => json!({"temp": name(&label, "temp")?}),
        _ => {
            return Err(invalid(
                field,
                "an entity is {:id \"e1...\"} or {:temp label}",
            ));
        }
    };
    fields.finish()?;
    Ok(entity)
}

pub(crate) fn entity_id(value: &Clj) -> Result<factlite::EntityId> {
    let mut fields = Fields::new(value, "entity")?;
    let id = text(&fields.required("id")?, "id")?;
    fields.finish()?;
    id.parse().map_err(library_error)
}

pub(crate) fn attr(value: &Clj) -> Result<AttrName> {
    AttrName::new(name(value, "attr")?).map_err(library_error)
}

fn revision_text(value: &Clj, field: &str) -> Result<String> {
    match unmeta(value) {
        Clj::Int(revision) if *revision >= 0 => Ok(revision.to_string()),
        Clj::BigInt(revision) => Ok(revision.clone()),
        other => Err(invalid(
            field,
            format!(
                "{field} must be a non-negative long, got {}",
                describe(other)
            ),
        )),
    }
}

/// A snapshot map as its wire object; the wire decoder validates identities.
pub(crate) fn snapshot_in(value: &Clj) -> Result<SnapshotId> {
    let mut fields = Fields::new(value, "snapshot")?;
    let version = match unmeta(&fields.required("version")?) {
        Clj::Int(version) => *version,
        other => {
            return Err(invalid(
                "version",
                format!("snapshot :version must be a long, got {}", describe(other)),
            ));
        }
    };
    let snapshot = json!({
        "version": version,
        "store": text(&fields.required("store")?, "store")?,
        "lineage": text(&fields.required("lineage")?, "lineage")?,
        "revision": revision_text(&fields.required("revision")?, "revision")?,
    });
    fields.finish()?;
    wire::decode_snapshot(&snapshot.to_string()).map_err(library_error)
}

pub(crate) fn revision_in(value: &Clj, field: &str) -> Result<factlite::Revision> {
    revision_text(value, field)?
        .parse::<u64>()
        .map(factlite::Revision::new)
        .map_err(|_| invalid(field, format!("{field} is out of range")))
}

fn definition_in(value: &Clj) -> Result<Json> {
    let mut fields = Fields::new(value, "schema")?;
    let unique = match name(&fields.required("unique")?, "unique")?.as_str() {
        "none" => "none",
        "unique-one" => "unique_one",
        _ => return Err(invalid("unique", "schema :unique is :none or :unique-one")),
    };
    let definition = json!({
        "name": name(&fields.required("name")?, "name")?,
        "type": name(&fields.required("type")?, "type")?,
        "cardinality": name(&fields.required("cardinality")?, "cardinality")?,
        "unique": unique,
    });
    fields.finish()?;
    Ok(definition)
}

fn change_in(value: &Clj) -> Result<Json> {
    let [op, entity, attribute, value] = exactly::<4>(value, "changes", "[op entity attr value]")?;
    let op = match unmeta(op) {
        Clj::Keyword(op) if op == "assert" || op == "retract" => op.clone(),
        _ => return Err(invalid("op", "a change op is :assert or :retract")),
    };
    Ok(json!({
        "op": op,
        "entity": entity_in(entity, "entity")?,
        "attr": name(attribute, "attr")?,
        "value": value_in(value, "value")?,
    }))
}

fn expectation_in(value: &Clj) -> Result<Json> {
    let [entity, attribute, values] = exactly::<3>(value, "expect", "[entity attr values]")?;
    let values = match unmeta(values) {
        Clj::Vector(values) | Clj::List(values) | Clj::Set(values) => values,
        other => {
            return Err(invalid(
                "values",
                format!(
                    "expected values must be a vector or set, got {}",
                    describe(other)
                ),
            ));
        }
    };
    Ok(json!({
        "entity": entity_in(entity, "entity")?,
        "attr": name(attribute, "attr")?,
        "values": values.iter().map(|value| value_in(value, "value")).collect::<Result<Vec<_>>>()?,
    }))
}

/// Keyword- or string-keyed scalar maps (metadata, params) as wire objects.
fn named_values(value: &Clj, field: &'static str) -> Result<Json> {
    let Clj::Map(entries) = unmeta(value) else {
        return Err(invalid(field, format!("{field} must be a map")));
    };
    let mut object = Map::new();
    for (key, value) in entries {
        let key = name(key, field)?;
        if object
            .insert(key.clone(), value_in(value, field)?)
            .is_some()
        {
            return Err(invalid(field, format!("duplicate {field} key {key}")));
        }
    }
    Ok(Json::Object(object))
}

fn version_in(fields: &mut Fields) -> Result<Json> {
    match fields.take("version").as_ref().map(unmeta) {
        None => Ok(json!(1)),
        Some(Clj::Int(version)) => Ok(json!(version)),
        Some(other) => Err(invalid(
            "version",
            format!(":version must be a long, got {}", describe(other)),
        )),
    }
}

/// A transaction map as a version-1 wire request.
pub(crate) fn tx_in(value: &Clj) -> Result<factlite::Tx> {
    let mut fields = Fields::new(value, "transaction")?;
    let mut object = Map::new();
    object.insert("version".into(), version_in(&mut fields)?);
    if let Some(schema) = fields.take("schema") {
        let schema = items(&schema, "schema")?
            .iter()
            .map(definition_in)
            .collect::<Result<Vec<_>>>()?;
        object.insert("schema".into(), json!(schema));
    }
    if let Some(labels) = fields.take("new-entities") {
        let labels = items(&labels, "new-entities")?
            .iter()
            .map(|label| name(label, "new-entities"))
            .collect::<Result<Vec<_>>>()?;
        object.insert("new_entities".into(), json!(labels));
    }
    if let Some(changes) = fields.take("changes") {
        let changes = items(&changes, "changes")?
            .iter()
            .map(change_in)
            .collect::<Result<Vec<_>>>()?;
        object.insert("changes".into(), json!(changes));
    }
    if let Some(expect) = fields.take("expect") {
        let expect = items(&expect, "expect")?
            .iter()
            .map(expectation_in)
            .collect::<Result<Vec<_>>>()?;
        object.insert("expect".into(), json!(expect));
    }
    if let Some(metadata) = fields.take("metadata") {
        object.insert("metadata".into(), named_values(&metadata, "metadata")?);
    }
    if let Some(head) = fields.take("expected-head") {
        let head = snapshot_in(&head)?;
        let head: Json = serde_json::from_str(&wire::encode_snapshot(&head))
            .map_err(|error| invalid("expected-head", error.to_string()))?;
        object.insert("expected_head".into(), head);
    }
    if let Some(key) = fields.take("idempotency-key") {
        object.insert(
            "idempotency_key".into(),
            json!(text(&key, "idempotency-key")?),
        );
    }
    fields.finish()?;
    wire::decode_tx(&Json::Object(object).to_string()).map_err(library_error)
}

fn variable(value: &Clj, field: &str) -> Result<String> {
    match unmeta(value) {
        Clj::Symbol(symbol) => symbol.strip_prefix('?').map(str::to_owned),
        _ => None,
    }
    .ok_or_else(|| invalid(field, format!("{field} entries are ?variables")))
}

/// `?x` is a variable, `$x` an input, and anything else a constant value.
fn term_in(value: &Clj) -> Result<Json> {
    if let Clj::Symbol(symbol) = unmeta(value) {
        return match (symbol.strip_prefix('?'), symbol.strip_prefix('$')) {
            (Some(var), _) => Ok(json!({"var": var})),
            (_, Some(param)) => Ok(json!({"param": param})),
            _ => Err(invalid(
                "term",
                format!("query symbol {symbol} must start with ? (variable) or $ (input)"),
            )),
        };
    }
    value_in(value, "term")
}

fn predicate_in(value: &Clj) -> Result<Json> {
    let [op, left, right] = exactly::<3>(value, "where", "(op left right)")?;
    let op = match unmeta(op) {
        Clj::Symbol(op) => match op.as_str() {
            "=" => "eq",
            "not=" => "ne",
            "<" => "lt",
            "<=" => "le",
            ">" => "gt",
            ">=" => "ge",
            _ => return Err(invalid("where", format!("unknown comparison {op}"))),
        },
        _ => {
            return Err(invalid(
                "where",
                "a predicate is (op left right) with op =, not=, <, <=, > or >=",
            ));
        }
    };
    Ok(json!({"left": term_in(left)?, "op": op, "right": term_in(right)?}))
}

/// A query map and optional params map as a version-1 wire envelope.
pub(crate) fn query_in(
    query: &Clj,
    params: Option<&Clj>,
) -> Result<(factlite::Query, factlite::Params)> {
    let mut fields = Fields::new(query, "query")?;
    let mut inner = Map::new();
    if let Some(inputs) = fields.take("inputs") {
        let Clj::Map(entries) = unmeta(&inputs) else {
            return Err(invalid("inputs", ":inputs must be a map"));
        };
        let mut object = Map::new();
        for (key, ty) in entries {
            object.insert(name(key, "inputs")?, json!(name(ty, "inputs")?));
        }
        inner.insert("inputs".into(), Json::Object(object));
    }
    if let Some(patterns) = fields.take("match") {
        let patterns = items(&patterns, "match")?
            .iter()
            .map(|pattern| {
                let [entity, attribute, value] =
                    exactly::<3>(pattern, "match", "[entity attr value]")?;
                Ok(json!([
                    term_in(entity)?,
                    name(attribute, "attr")?,
                    term_in(value)?
                ]))
            })
            .collect::<Result<Vec<_>>>()?;
        inner.insert("match".into(), json!(patterns));
    }
    if let Some(predicates) = fields.take("where") {
        let predicates = items(&predicates, "where")?
            .iter()
            .map(predicate_in)
            .collect::<Result<Vec<_>>>()?;
        inner.insert("where".into(), json!(predicates));
    }
    if let Some(select) = fields.take("select") {
        let select = items(&select, "select")?
            .iter()
            .map(|var| variable(var, "select"))
            .collect::<Result<Vec<_>>>()?;
        inner.insert("select".into(), json!(select));
    }
    if let Some(order) = fields.take("order") {
        let order = items(&order, "order")?
            .iter()
            .map(|entry| {
                let [field, direction] = exactly::<2>(entry, "order", "[?variable direction]")?;
                let direction = match unmeta(direction) {
                    Clj::Keyword(direction) if direction == "asc" || direction == "desc" => {
                        direction.clone()
                    }
                    _ => return Err(invalid("order", "order direction is :asc or :desc")),
                };
                Ok(json!({"field": variable(field, "order")?, "direction": direction}))
            })
            .collect::<Result<Vec<_>>>()?;
        inner.insert("order".into(), json!(order));
    }
    if let Some(limit) = fields.take("limit") {
        match unmeta(&limit) {
            Clj::Int(limit) => inner.insert("limit".into(), json!(limit)),
            other => {
                return Err(invalid(
                    "limit",
                    format!(":limit must be a long, got {}", describe(other)),
                ));
            }
        };
    }
    fields.finish()?;
    let params = match params {
        None | Some(Clj::Nil) => json!({}),
        Some(params) => named_values(params, "params")?,
    };
    let envelope = json!({"version": 1, "query": inner, "params": params});
    wire::decode_query(&envelope.to_string()).map_err(library_error)
}

/// `{:entity {:id ...} :attr :a/b :value v}`, each key optional.
pub(crate) fn filter_in(value: Option<&Clj>) -> Result<FactFilter> {
    let mut fields = Fields::optional(value, "filter")?;
    let entity = fields.take("entity").as_ref().map(entity_id).transpose()?;
    let attribute = fields.take("attr").as_ref().map(attr).transpose()?;
    let value = fields
        .take("value")
        .as_ref()
        .map(|value| {
            wire::decode_value(&value_in(value, "value")?.to_string()).map_err(library_error)
        })
        .transpose()?;
    fields.finish()?;
    Ok(FactFilter {
        entity,
        attribute,
        value,
    })
}

// ---------------------------------------------------------------------------
// Writing Clojure output

fn ref_id(id: &str) -> Clj {
    map(vec![("id", string(id))])
}

fn long(number: u64) -> Clj {
    i64::try_from(number).map_or_else(|_| Clj::BigInt(number.to_string()), Clj::Int)
}

pub(crate) fn value_out(value: &Value) -> Clj {
    match value {
        Value::Bool(value) => Clj::Bool(*value),
        Value::I64(value) => Clj::Int(*value),
        Value::F64(value) => Clj::Float(value.get()),
        Value::String(value) => string(value),
        Value::Bytes(value) => Clj::Bytes(value.clone()),
        Value::Ref(id) => ref_id(&id.to_string()),
    }
}

fn attr_out(attribute: &AttrName) -> Clj {
    kw(attribute.as_str())
}

pub(crate) fn snapshot_out(snapshot: &SnapshotId) -> Clj {
    map(vec![
        ("version", Clj::Int(1)),
        ("store", string(&snapshot.store().to_string())),
        ("lineage", string(&snapshot.lineage().to_string())),
        ("revision", long(snapshot.revision().get())),
    ])
}

fn type_out(ty: ValueType) -> Clj {
    kw(match ty {
        ValueType::Bool => "bool",
        ValueType::I64 => "i64",
        ValueType::F64 => "f64",
        ValueType::String => "string",
        ValueType::Bytes => "bytes",
        ValueType::Ref => "ref",
    })
}

pub(crate) fn definition_out(definition: &AttributeDefinition) -> Clj {
    map(vec![
        ("name", attr_out(definition.name())),
        ("type", type_out(definition.value_type())),
        (
            "cardinality",
            kw(match definition.cardinality() {
                Cardinality::One => "one",
                Cardinality::Many => "many",
            }),
        ),
        (
            "unique",
            kw(match definition.uniqueness() {
                Uniqueness::None => "none",
                Uniqueness::UniqueOne => "unique-one",
            }),
        ),
    ])
}

fn op_out(operation: Operation) -> Clj {
    kw(match operation {
        Operation::Assert => "assert",
        Operation::Retract => "retract",
    })
}

pub(crate) fn fact_out(fact: &Fact) -> Clj {
    Clj::Vector(vec![
        ref_id(&fact.entity.to_string()),
        attr_out(&fact.attribute),
        value_out(&fact.value),
    ])
}

fn delta_out(delta: &FactDelta) -> Clj {
    Clj::Vector(vec![
        op_out(delta.operation),
        ref_id(&delta.fact.entity.to_string()),
        attr_out(&delta.fact.attribute),
        value_out(&delta.fact.value),
    ])
}

fn metadata_out(metadata: &std::collections::BTreeMap<String, Value>) -> Clj {
    Clj::Map(
        metadata
            .iter()
            .map(|(key, value)| (kw(key), value_out(value)))
            .collect(),
    )
}

pub(crate) fn report_out(report: &TxReport) -> Clj {
    map(vec![
        ("before", snapshot_out(&report.before)),
        ("after", snapshot_out(&report.after)),
        (
            "tempids",
            Clj::Map(
                report
                    .tempids
                    .iter()
                    .map(|(label, id)| (kw(label.as_str()), ref_id(&id.to_string())))
                    .collect(),
            ),
        ),
        (
            "created-entities",
            Clj::Vector(
                report
                    .created_entities
                    .iter()
                    .map(|id| ref_id(&id.to_string()))
                    .collect(),
            ),
        ),
        (
            "schema-added",
            Clj::Vector(report.schema_added.iter().map(definition_out).collect()),
        ),
        (
            "effective-delta",
            Clj::Vector(report.effective_delta.iter().map(delta_out).collect()),
        ),
        ("metadata", metadata_out(&report.metadata)),
    ])
}

/// A pulled entity is its reference map plus present attributes; attribute
/// names always have a namespace, so they never collide with `:id`.
pub(crate) fn entity_out(entity: Option<&Entity>) -> Clj {
    let Some(entity) = entity else {
        return Clj::Nil;
    };
    let mut entries = vec![(kw("id"), string(&entity.id.to_string()))];
    for (name, value) in &entity.attributes {
        let value = match value {
            PulledValue::One(value) => value_out(value),
            PulledValue::Many(values) => Clj::Vector(values.iter().map(value_out).collect()),
        };
        entries.push((attr_out(name), value));
    }
    Clj::Map(entries)
}

pub(crate) fn event_out(event: &FactEvent) -> Clj {
    map(vec![
        ("revision", long(event.revision.get())),
        ("ordinal", long(event.ordinal)),
        ("op", op_out(event.operation)),
        ("entity", ref_id(&event.fact.entity.to_string())),
        ("attr", attr_out(&event.fact.attribute)),
        ("value", value_out(&event.fact.value)),
    ])
}

pub(crate) fn result_out(result: &QueryResult) -> Clj {
    map(vec![
        (
            "columns",
            Clj::Vector(
                result
                    .columns
                    .iter()
                    .map(|name| kw(name.as_str()))
                    .collect(),
            ),
        ),
        (
            "rows",
            Clj::Vector(
                result
                    .rows
                    .iter()
                    .map(|row| Clj::Vector(row.iter().map(value_out).collect()))
                    .collect(),
            ),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn sym(name: &str) -> Clj {
        Clj::Symbol(name.to_owned())
    }
    fn vector(items: Vec<Clj>) -> Clj {
        Clj::Vector(items)
    }
    fn code(error: PodError) -> String {
        error.data().map_or(String::new(), |data| {
            serde_json::to_string(data).unwrap_or_default()
        })
    }
    const ID: &str = "e1.0123456789abcdef0123456789abcdef.0000000000000002";

    fn round_trip(value: &Clj) -> Clj {
        let wire_value = value_in(value, "value").unwrap();
        value_out(&wire::decode_value(&wire_value.to_string()).unwrap())
    }

    #[test]
    fn each_type_has_one_representation_in_both_directions() {
        for value in [
            Clj::Bool(true),
            Clj::Int(i64::MIN),
            Clj::Int(i64::MAX),
            Clj::Float(1.0),
            Clj::Float(5e-324),
            Clj::Float(f64::MAX),
            string("雪\0x"),
            Clj::Bytes(vec![0, 1, 255]),
            ref_id(ID),
        ] {
            assert_eq!(round_trip(&value), value);
        }
        assert_eq!(round_trip(&Clj::Float(-0.0)), Clj::Float(0.0));
        assert!(
            matches!(round_trip(&Clj::Float(-0.0)), Clj::Float(zero) if zero.is_sign_positive())
        );
        // A long and a double never meet: the tags differ on the wire.
        assert_ne!(
            value_in(&Clj::Int(1), "v").unwrap(),
            value_in(&Clj::Float(1.0), "v").unwrap()
        );
    }

    #[test]
    fn unsupported_values_are_rejected_not_coerced() {
        for value in [
            Clj::Nil,
            kw("done"),
            sym("x"),
            Clj::BigInt("1".into()),
            Clj::BigDec("1.5".into()),
            Clj::Ratio {
                numerator: "1".into(),
                denominator: "3".into(),
            },
            Clj::Char('a'),
            Clj::Inst(0),
            Clj::Uuid("00000000-0000-0000-0000-000000000000".into()),
            Clj::Uri("https://example.com".into()),
            vector(vec![]),
            Clj::Set(vec![]),
            Clj::List(vec![]),
            Clj::Float(f64::NAN),
            Clj::Float(f64::INFINITY),
            map(vec![("id", string(ID)), ("temp", string("x"))]),
            map(vec![("ident", string(ID))]),
        ] {
            let error = value_in(&value, "value").unwrap_err();
            assert!(code(error).contains("InvalidRequest"), "{value:?}");
        }
    }

    fn create_clj() -> Clj {
        map(vec![
            ("idempotency-key", string("demo-create-v1")),
            (
                "schema",
                vector(vec![
                    map(vec![
                        ("name", kw("person/name")),
                        ("type", kw("string")),
                        ("cardinality", kw("one")),
                        ("unique", kw("none")),
                    ]),
                    map(vec![
                        ("name", kw("task/owner")),
                        ("type", kw("ref")),
                        ("cardinality", kw("one")),
                        ("unique", kw("unique-one")),
                    ]),
                ]),
            ),
            ("new-entities", vector(vec![kw("person"), string("task")])),
            (
                "changes",
                vector(vec![
                    vector(vec![
                        kw("assert"),
                        map(vec![("temp", kw("person"))]),
                        kw("person/name"),
                        string("Ada"),
                    ]),
                    vector(vec![
                        kw("assert"),
                        map(vec![("temp", string("task"))]),
                        kw("task/owner"),
                        map(vec![("temp", string("person"))]),
                    ]),
                ]),
            ),
            ("metadata", map(vec![("source", string("design-example"))])),
        ])
    }

    #[test]
    fn pod_and_cli_requests_share_canonical_retry_identity() {
        let json = r#"{"version":1,"idempotency_key":"demo-create-v1",
          "schema":[{"name":"person/name","type":"string","cardinality":"one","unique":"none"},
                    {"name":"task/owner","type":"ref","cardinality":"one","unique":"unique_one"}],
          "new_entities":["person","task"],
          "changes":[{"op":"assert","entity":{"temp":"person"},"attr":"person/name","value":{"string":"Ada"}},
                     {"op":"assert","entity":{"temp":"task"},"attr":"task/owner","value":{"ref":{"temp":"person"}}}],
          "metadata":{"source":{"string":"design-example"}}}"#;
        let cli = wire::decode_tx(json).unwrap();
        let pod = tx_in(&create_clj()).unwrap();
        assert_eq!(pod, cli);
        assert_eq!(pod.canonical_request(), cli.canonical_request());
    }

    #[test]
    fn transaction_maps_are_strict() {
        let with = |key: &str, value: Clj| {
            let Clj::Map(mut entries) = create_clj() else {
                unreachable!()
            };
            entries.push((kw(key), value));
            tx_in(&Clj::Map(entries))
        };
        for (key, value) in [
            ("chnges", vector(vec![])),
            ("version", Clj::Int(2)),
            ("version", string("1")),
            ("idempotency-key", Clj::Int(1)),
        ] {
            // An appended duplicate key is rejected too.
            assert!(
                code(with(key, value).unwrap_err()).contains("InvalidRequest"),
                "{key}"
            );
        }
        let string_keys = Clj::Map(vec![(string("changes"), vector(vec![]))]);
        assert!(tx_in(&string_keys).is_err());
        let colliding = map(vec![(
            "metadata",
            Clj::Map(vec![(kw("a"), Clj::Int(1)), (string("a"), Clj::Int(2))]),
        )]);
        assert!(code(tx_in(&colliding).unwrap_err()).contains("duplicate"));
        let bad_op = map(vec![(
            "changes",
            vector(vec![vector(vec![
                kw("upsert"),
                ref_id(ID),
                kw("a/b"),
                Clj::Int(1),
            ])]),
        )]);
        assert!(tx_in(&bad_op).is_err());
        // Temporary references are confined to changes by the wire decoder.
        let temp_expect = map(vec![(
            "expect",
            vector(vec![vector(vec![
                ref_id(ID),
                kw("a/b"),
                vector(vec![map(vec![("temp", string("x"))])]),
            ])]),
        )]);
        assert!(tx_in(&temp_expect).is_err());
    }

    #[test]
    fn query_terms_use_variable_and_input_symbols() {
        let query = map(vec![
            ("inputs", map(vec![("wanted", kw("string"))])),
            (
                "match",
                vector(vec![
                    vector(vec![sym("?task"), kw("task/status"), sym("$wanted")]),
                    vector(vec![sym("?task"), kw("task/owner"), ref_id(ID)]),
                ]),
            ),
            (
                "where",
                vector(vec![Clj::List(vec![sym("not="), sym("?task"), ref_id(ID)])]),
            ),
            ("select", vector(vec![sym("?task")])),
            (
                "order",
                vector(vec![vector(vec![sym("?task"), kw("desc")])]),
            ),
            ("limit", Clj::Int(5)),
        ]);
        let params = map(vec![("wanted", string("doing"))]);
        let json = format!(
            r#"{{"version":1,"query":{{"inputs":{{"wanted":"string"}},
              "match":[[{{"var":"task"}},"task/status",{{"param":"wanted"}}],
                       [{{"var":"task"}},"task/owner",{{"ref":{{"id":"{ID}"}}}}]],
              "where":[{{"left":{{"var":"task"}},"op":"ne","right":{{"ref":{{"id":"{ID}"}}}}}}],
              "select":["task"],"order":[{{"field":"task","direction":"desc"}}],"limit":5}},
              "params":{{"wanted":{{"string":"doing"}}}}}}"#
        );
        assert_eq!(
            query_in(&query, Some(&params)).unwrap(),
            wire::decode_query(&json).unwrap()
        );
        for bad in [
            map(vec![("select", vector(vec![sym("task")]))]),
            map(vec![(
                "match",
                vector(vec![vector(vec![sym("x"), kw("a/b"), sym("?v")])]),
            )]),
            map(vec![(
                "where",
                vector(vec![Clj::List(vec![sym("like"), sym("?a"), string("x")])]),
            )]),
            map(vec![("limit", Clj::Int(-1))]),
            map(vec![("selct", vector(vec![]))]),
        ] {
            assert!(query_in(&bad, None).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn snapshots_round_trip_and_validate() {
        let snapshot: SnapshotId =
            "d1.0123456789abcdef0123456789abcdef.fedcba9876543210fedcba9876543210.7"
                .parse()
                .unwrap();
        assert_eq!(snapshot_in(&snapshot_out(&snapshot)).unwrap(), snapshot);
        let Clj::Map(mut entries) = snapshot_out(&snapshot) else {
            unreachable!()
        };
        entries.push((kw("extra"), Clj::Int(1)));
        assert!(snapshot_in(&Clj::Map(entries)).is_err());
        let negative = map(vec![
            ("version", Clj::Int(1)),
            ("store", string(&snapshot.store().to_string())),
            ("lineage", string(&snapshot.lineage().to_string())),
            ("revision", Clj::Int(-1)),
        ]);
        assert!(snapshot_in(&negative).is_err());
        assert_eq!(long(u64::MAX), Clj::BigInt(u64::MAX.to_string()));
    }

    #[test]
    fn library_errors_keep_code_context_and_outcome() {
        let error = wire::decode_tx(r#"{"version":1,"changes":[{"op":"assert"}]}"#).unwrap_err();
        let data = serde_json::to_string(library_error(error).data().unwrap()).unwrap();
        assert!(
            data.contains("InvalidRequest") && data.contains("context"),
            "{data}"
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1024))]
        #[test]
        fn random_values_round_trip_exactly(
            choice in 0_u8..6,
            number in any::<i64>(),
            float in any::<f64>().prop_filter("finite", |f| f.is_finite()),
            text in ".*",
            bytes in proptest::collection::vec(any::<u8>(), 0..32),
            allocation in 1_u64..,
        ) {
            let value = match choice {
                0 => Clj::Bool(number % 2 == 0),
                1 => Clj::Int(number),
                2 => Clj::Float(float),
                3 => Clj::String(text),
                4 => Clj::Bytes(bytes),
                _ => ref_id(&format!("e1.0123456789abcdef0123456789abcdef.{allocation:016x}")),
            };
            let expected = match value {
                Clj::Float(0.0) => Clj::Float(0.0),
                ref other => other.clone(),
            };
            prop_assert_eq!(round_trip(&value), expected);
        }
    }
}
