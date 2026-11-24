//! The parts of a read request: which entity, attributes, facts and revisions.
//!
//! The CLI takes these from options. An interface that carries a read as one
//! JSON object uses these decoders on each member's text, so every interface
//! accepts the same spellings.

use serde_json::{Value as Json, json};

use super::{Object, concrete_value, strict, text, value_json};
use crate::{AttrName, EntityId, EntityTerm, Error, FactFilter, Result, Revision};

fn entity_id(value: Json) -> Result<EntityId> {
    match super::parse_entity(value)? {
        EntityTerm::Id(id) => Ok(id),
        EntityTerm::Temp(_) | EntityTerm::Lookup(..) => Err(Error::invalid(
            "entity",
            "a read names an existing entity by id",
        )),
    }
}
fn attr(value: Json) -> Result<AttrName> {
    text(value, "attr")?.parse()
}

/// Decode an existing entity, `{"id":"e1..."}`. Temporary labels and lookup
/// references belong to transactions and are refused.
pub fn decode_entity_id(input: &str) -> Result<EntityId> {
    entity_id(strict::parse(input)?)
}
/// Encode an existing entity as `{"id":"e1..."}`.
pub fn encode_entity_id(entity: &EntityId) -> String {
    json!({"id":entity.to_string()}).to_string()
}

/// Decode one qualified attribute name, a JSON string.
pub fn decode_attr(input: &str) -> Result<AttrName> {
    attr(strict::parse(input)?)
}
/// Encode one qualified attribute name as a JSON string.
pub fn encode_attr(attribute: &AttrName) -> String {
    Json::String(attribute.as_str().to_owned()).to_string()
}

/// Decode a projection: an array of qualified attribute names, in order. An
/// empty array projects no fields.
pub fn decode_attrs(input: &str) -> Result<Vec<AttrName>> {
    match strict::parse(input)? {
        Json::Array(names) => names.into_iter().map(attr).collect(),
        _ => Err(Error::invalid("attrs", "expected an array")),
    }
}
/// Encode a projection as an array of qualified attribute names, in order.
pub fn encode_attrs(attributes: &[AttrName]) -> String {
    json!(attributes.iter().map(AttrName::as_str).collect::<Vec<_>>()).to_string()
}

/// Decode a fact filter: an object with any of `entity`, `attr` and `value`.
/// An absent member matches everything; `{}` is the empty filter. Explicit
/// null is not a default.
pub fn decode_filter(input: &str) -> Result<FactFilter> {
    let mut object = Object::new(strict::parse(input)?, "filter")?;
    let filter = FactFilter {
        entity: object.optional("entity").map(entity_id).transpose()?,
        attribute: object.optional("attr").map(attr).transpose()?,
        value: object.optional("value").map(concrete_value).transpose()?,
    };
    object.finish()?;
    Ok(filter)
}
/// Encode a fact filter, omitting absent restrictions.
pub fn encode_filter(filter: &FactFilter) -> String {
    let mut object = json!({});
    if let Some(entity) = &filter.entity {
        object["entity"] = json!({"id":entity.to_string()});
    }
    if let Some(attribute) = &filter.attribute {
        object["attr"] = json!(attribute.as_str());
    }
    if let Some(value) = &filter.value {
        object["value"] = value_json(value);
    }
    object.to_string()
}

/// Decode a revision: a JSON string of canonical unsigned decimal digits,
/// with no sign and no leading zero.
pub fn decode_revision(input: &str) -> Result<Revision> {
    text(strict::parse(input)?, "revision")?.parse()
}
/// Encode a revision as a JSON string of decimal digits.
pub fn encode_revision(revision: Revision) -> String {
    Json::String(revision.to_string()).to_string()
}
