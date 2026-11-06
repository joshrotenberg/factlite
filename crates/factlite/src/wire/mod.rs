//! Version-one public JSON interchange, separate from storage codecs.
//!
//! All objects reject duplicate keys and unknown fields. Values have exactly one
//! type tag. Omitted optional transaction fields default to empty collections or
//! no key/head fence; explicit null is not a default. Integers/revisions use
//! decimal strings, floats finite round-trippable strings, bytes padded base64.

mod output;
mod query;
pub use query::{decode_query, encode_query, encode_query_result};
mod strict;
pub use output::{
    encode_entity, encode_error, encode_facts, encode_history, encode_lookup, encode_report,
    encode_schema, encode_transactions,
};

use base64::{
    Engine, alphabet,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
};
use serde_json::{Map, Value as Json, json};

use crate::{
    AttributeDefinition, Cardinality, Change, EntityTerm, Error, ErrorCode, ErrorContext,
    Expectation, Operation, Result, Revision, SnapshotId, Tx, Uniqueness, Value, ValueTerm,
    ValueType,
};

struct Object {
    fields: Map<String, Json>,
    field: &'static str,
}
impl Object {
    fn new(value: Json, field: &'static str) -> Result<Self> {
        match value {
            Json::Object(fields) => Ok(Self { fields, field }),
            _ => Err(Error::invalid(field, "expected an object")),
        }
    }
    fn required(&mut self, key: &str) -> Result<Json> {
        self.fields
            .remove(key)
            .ok_or_else(|| Error::invalid(self.field, format!("missing field: {key}")))
    }
    fn optional(&mut self, key: &str) -> Option<Json> {
        self.fields.remove(key)
    }
    fn array(&mut self, key: &str) -> Result<Vec<Json>> {
        match self.optional(key) {
            None => Ok(Vec::new()),
            Some(Json::Array(values)) => Ok(values),
            _ => Err(Error::invalid(self.field, format!("expected array: {key}"))),
        }
    }
    fn version(&mut self) -> Result<()> {
        if self.required("version")?.as_u64() != Some(1) {
            return Err(Error::invalid("version", "unsupported wire version"));
        }
        Ok(())
    }
    fn finish(self) -> Result<()> {
        if let Some(key) = self.fields.keys().next() {
            return Err(Error::invalid(self.field, format!("unknown field: {key}")));
        }
        Ok(())
    }
}
/// The wire name of each value type.
pub(crate) fn type_name(ty: ValueType) -> &'static str {
    match ty {
        ValueType::Bool => "bool",
        ValueType::I64 => "i64",
        ValueType::F64 => "f64",
        ValueType::String => "string",
        ValueType::Bytes => "bytes",
        ValueType::Ref => "ref",
        ValueType::Keyword => "keyword",
        ValueType::Inst => "inst",
    }
}
pub(crate) fn type_from_name(name: &str) -> Option<ValueType> {
    [
        ValueType::Bool,
        ValueType::I64,
        ValueType::F64,
        ValueType::String,
        ValueType::Bytes,
        ValueType::Ref,
        ValueType::Keyword,
        ValueType::Inst,
    ]
    .into_iter()
    .find(|ty| type_name(*ty) == name)
}
fn signed_decimal(value: Json, field: &'static str) -> Result<i64> {
    let decimal = text(value, field)?;
    let digits = decimal.strip_prefix(['+', '-']).unwrap_or(&decimal);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(Error::invalid(field, "expected signed decimal digits"));
    }
    decimal
        .parse()
        .map_err(|_| Error::invalid(field, "signed integer overflow"))
}
fn text(value: Json, field: &'static str) -> Result<String> {
    match value {
        Json::String(value) => Ok(value),
        _ => Err(Error::invalid(field, "expected a string")),
    }
}
fn parse_snapshot(value: Json) -> Result<SnapshotId> {
    let result = (|| {
        let mut object = Object::new(value, "snapshot")?;
        object.version()?;
        let store = text(object.required("store")?, "store")?.parse()?;
        let lineage = text(object.required("lineage")?, "lineage")?.parse()?;
        let revision = text(object.required("revision")?, "revision")?;
        if revision.is_empty() || !revision.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(Error::invalid(
                "revision",
                "expected unsigned decimal digits",
            ));
        }
        let revision = Revision::new(
            revision
                .parse()
                .map_err(|_| Error::invalid("revision", "revision overflow"))?,
        );
        object.finish()?;
        Ok(SnapshotId::new(store, lineage, revision))
    })();
    result.map_err(|error: Error| {
        Error::new(
            ErrorCode::InvalidSnapshot,
            ErrorContext::InvalidInput { field: "snapshot" },
            error.to_string(),
        )
    })
}
pub(crate) fn snapshot_json(snapshot: &SnapshotId) -> Json {
    json!({"version":1,"store":snapshot.store().to_string(),"lineage":snapshot.lineage().to_string(),"revision":snapshot.revision().to_string()})
}

/// Parse a portable full token. Store availability is checked by `Store::db_at`.
pub fn decode_snapshot(input: &str) -> Result<SnapshotId> {
    parse_snapshot(strict::parse(input)?)
}
/// Encode a portable token with canonical decimal revision spelling.
pub fn encode_snapshot(snapshot: &SnapshotId) -> String {
    snapshot_json(snapshot).to_string()
}

fn parse_entity(value: Json) -> Result<EntityTerm> {
    let mut object = Object::new(value, "entity")?;
    let entity = match (object.optional("id"), object.optional("temp")) {
        (Some(value), None) => EntityTerm::Id(text(value, "id")?.parse()?),
        (None, Some(value)) => EntityTerm::Temp(crate::TempId::new(text(value, "temp")?)?),
        _ => {
            return Err(Error::invalid(
                "entity",
                "expected exactly one id or temp tag",
            ));
        }
    };
    object.finish()?;
    Ok(entity)
}
fn entity_json(entity: &EntityTerm) -> Json {
    match entity {
        EntityTerm::Id(id) => json!({"id":id.to_string()}),
        EntityTerm::Temp(label) => json!({"temp":label.as_str()}),
    }
}

fn parse_value(value: Json, allow_temp: bool) -> Result<ValueTerm> {
    let Json::Object(fields) = value else {
        return Err(Error::invalid("value", "expected one type tag"));
    };
    if fields.len() != 1 {
        return Err(Error::invalid("value", "expected exactly one type tag"));
    }
    let (tag, value) = fields.into_iter().next().expect("one field");
    let value = match tag.as_str() {
        "bool" => Value::Bool(
            value
                .as_bool()
                .ok_or_else(|| Error::invalid("bool", "expected a boolean"))?,
        ),
        "i64" => Value::I64(signed_decimal(value, "i64")?),
        "inst" => Value::Inst(signed_decimal(value, "inst")?),
        "keyword" => Value::Keyword(crate::Keyword::new(text(value, "keyword")?)?),
        "f64" => Value::f64(
            text(value, "f64")?
                .parse()
                .map_err(|_| Error::invalid("f64", "expected a finite decimal float"))?,
        )?,
        "string" => Value::String(text(value, "string")?),
        "bytes" => {
            // Accept standard base64 with or without padding; emit padded standard.
            // Nonzero trailing bits, whitespace, and URL-safe alphabet are rejected.
            let decoder = GeneralPurpose::new(
                &alphabet::STANDARD,
                GeneralPurposeConfig::new()
                    .with_decode_padding_mode(DecodePaddingMode::Indifferent),
            );
            Value::Bytes(
                decoder
                    .decode(text(value, "bytes")?)
                    .map_err(|error| Error::invalid("bytes", error.to_string()))?,
            )
        }
        "ref" => match parse_entity(value)? {
            EntityTerm::Id(id) => Value::Ref(id),
            EntityTerm::Temp(label) if allow_temp => return Ok(ValueTerm::TempRef(label)),
            EntityTerm::Temp(_) => {
                return Err(Error::invalid(
                    "ref",
                    "temporary references are only allowed in changes",
                ));
            }
        },
        _ => return Err(Error::invalid("value", format!("unknown type tag: {tag}"))),
    };
    Ok(ValueTerm::Value(value))
}
fn concrete_value(value: Json) -> Result<Value> {
    match parse_value(value, false)? {
        ValueTerm::Value(value) => Ok(value),
        ValueTerm::TempRef(_) => unreachable!("temporary references rejected"),
    }
}
pub(crate) fn value_json(value: &Value) -> Json {
    match value {
        Value::Bool(value) => json!({"bool":value}),
        Value::I64(value) => json!({"i64":value.to_string()}),
        Value::F64(value) => json!({"f64":value.get().to_string()}),
        Value::String(value) => json!({"string":value}),
        Value::Bytes(value) => {
            json!({"bytes":base64::engine::general_purpose::STANDARD.encode(value)})
        }
        Value::Ref(id) => json!({"ref":{"id":id.to_string()}}),
        Value::Keyword(keyword) => json!({"keyword":keyword.as_str()}),
        Value::Inst(millis) => json!({"inst":millis.to_string()}),
    }
}
fn value_term_json(value: &ValueTerm) -> Json {
    match value {
        ValueTerm::Value(value) => value_json(value),
        ValueTerm::TempRef(label) => json!({"ref":{"temp":label.as_str()}}),
    }
}
/// Decode one concrete tagged value, including allocated references.
pub fn decode_value(input: &str) -> Result<Value> {
    concrete_value(strict::parse(input)?)
}
/// Encode a concrete tagged value, preserving its exact type.
pub fn encode_value(value: &Value) -> String {
    value_json(value).to_string()
}

fn parse_definition(value: Json) -> Result<AttributeDefinition> {
    let mut object = Object::new(value, "schema")?;
    let name = text(object.required("name")?, "name")?.parse()?;
    let ty = type_from_name(&text(object.required("type")?, "type")?)
        .ok_or_else(|| Error::invalid("type", "unknown value type"))?;
    let cardinality = match text(object.required("cardinality")?, "cardinality")?.as_str() {
        "one" => Cardinality::One,
        "many" => Cardinality::Many,
        _ => return Err(Error::invalid("cardinality", "unknown cardinality")),
    };
    let unique = match text(object.required("unique")?, "unique")?.as_str() {
        "none" => Uniqueness::None,
        "unique_one" => Uniqueness::UniqueOne,
        _ => return Err(Error::invalid("unique", "unknown uniqueness policy")),
    };
    object.finish()?;
    AttributeDefinition::new(name, ty, cardinality, unique)
}
pub(crate) fn definition_json(definition: &AttributeDefinition) -> Json {
    let ty = type_name(definition.value_type());
    json!({"name":definition.name().as_str(),"type":ty,"cardinality":match definition.cardinality() { Cardinality::One => "one", Cardinality::Many => "many" },"unique":match definition.uniqueness() { Uniqueness::None => "none", Uniqueness::UniqueOne => "unique_one" }})
}

/// Decode a version-one request with schema-independent structural validation.
/// This never reads a database or resolves temporary labels against durable state.
pub fn decode_tx(input: &str) -> Result<Tx> {
    let mut object = Object::new(strict::parse(input)?, "transaction")?;
    object.version()?;
    let schema = object
        .array("schema")?
        .into_iter()
        .map(parse_definition)
        .collect::<Result<_>>()?;
    let new_entities = object
        .array("new_entities")?
        .into_iter()
        .map(|value| crate::TempId::new(text(value, "new_entities")?))
        .collect::<Result<_>>()?;
    let changes = object
        .array("changes")?
        .into_iter()
        .map(|value| {
            let mut object = Object::new(value, "change")?;
            let operation = match text(object.required("op")?, "op")?.as_str() {
                "assert" => Operation::Assert,
                "retract" => Operation::Retract,
                _ => return Err(Error::invalid("op", "unknown operation")),
            };
            let entity = parse_entity(object.required("entity")?)?;
            let attribute = text(object.required("attr")?, "attr")?.parse()?;
            let value = parse_value(object.required("value")?, true)?;
            object.finish()?;
            Ok(Change {
                operation,
                entity,
                attribute,
                value,
            })
        })
        .collect::<Result<_>>()?;
    let expectations = object
        .array("expect")?
        .into_iter()
        .map(|value| {
            let mut object = Object::new(value, "expectation")?;
            let EntityTerm::Id(entity) = parse_entity(object.required("entity")?)? else {
                return Err(Error::invalid("expect", "expected an existing entity"));
            };
            let attribute = text(object.required("attr")?, "attr")?.parse()?;
            let Json::Array(values) = object.required("values")? else {
                return Err(Error::invalid("values", "expected an array"));
            };
            let values = values
                .into_iter()
                .map(concrete_value)
                .collect::<Result<_>>()?;
            object.finish()?;
            Ok(Expectation {
                entity,
                attribute,
                values,
            })
        })
        .collect::<Result<_>>()?;
    let metadata = match object.optional("metadata") {
        None => Default::default(),
        Some(Json::Object(fields)) => fields
            .into_iter()
            .map(|(key, value)| Ok((key, concrete_value(value)?)))
            .collect::<Result<_>>()?,
        Some(_) => return Err(Error::invalid("metadata", "expected an object")),
    };
    let expected_head = object
        .optional("expected_head")
        .map(parse_snapshot)
        .transpose()?;
    let idempotency_key = object
        .optional("idempotency_key")
        .map(|value| text(value, "idempotency_key"))
        .transpose()?;
    object.finish()?;
    Tx {
        schema,
        new_entities,
        changes,
        expectations,
        metadata,
        expected_head,
        idempotency_key,
    }
    .validate()
}

/// Encode a complete request with explicit collection defaults. Absent optional
/// key/head fields are omitted. Operation and declaration arrays preserve order.
pub fn encode_tx(tx: &Tx) -> String {
    let mut object = json!({
        "version":1,
        "schema":tx.schema.iter().map(definition_json).collect::<Vec<_>>(),
        "new_entities":tx.new_entities.iter().map(|label| label.as_str()).collect::<Vec<_>>(),
        "changes":tx.changes.iter().map(|change| json!({"op":match change.operation { Operation::Retract => "retract", Operation::Assert => "assert" },"entity":entity_json(&change.entity),"attr":change.attribute.as_str(),"value":value_term_json(&change.value)})).collect::<Vec<_>>(),
        "expect":tx.expectations.iter().map(|expect| json!({"entity":{"id":expect.entity.to_string()},"attr":expect.attribute.as_str(),"values":expect.values.iter().map(value_json).collect::<Vec<_>>()})).collect::<Vec<_>>(),
        "metadata":tx.metadata.iter().map(|(key,value)| (key.clone(),value_json(value))).collect::<Map<_,_>>()
    });
    if let Some(head) = &tx.expected_head {
        object["expected_head"] = snapshot_json(head);
    }
    if let Some(key) = &tx.idempotency_key {
        object["idempotency_key"] = Json::String(key.clone());
    }
    object.to_string()
}
