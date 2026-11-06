//! Lossless version-one response values.

use serde_json::{Map, Value as Json, json};

use super::{definition_json, snapshot_json, value_json};
use crate::{
    CommitOutcome, Entity, Error, ErrorCode, ErrorContext, Fact, FactEvent, Operation, PulledValue,
    Schema, TxRecord, TxReport, ValueType,
};

fn operation(operation: Operation) -> &'static str {
    match operation {
        Operation::Retract => "retract",
        Operation::Assert => "assert",
    }
}
fn fact_json(fact: &Fact) -> Json {
    json!({"entity":{"id":fact.entity.to_string()},"attr":fact.attribute.as_str(),"value":value_json(&fact.value)})
}
fn report_json(report: &TxReport) -> Json {
    json!({"version":1,"before":snapshot_json(&report.before),"after":snapshot_json(&report.after),
        "tempids":report.tempids.iter().map(|(label,id)| (label.as_str().to_owned(),json!(id.to_string()))).collect::<Map<_,_>>(),
        "created_entities":report.created_entities.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
        "schema_added":report.schema_added.iter().map(definition_json).collect::<Vec<_>>(),
        "effective_delta":report.effective_delta.iter().map(|delta| { let mut fact = fact_json(&delta.fact); fact["op"] = json!(operation(delta.operation)); fact }).collect::<Vec<_>>(),
        "metadata":report.metadata.iter().map(|(key,value)| (key.clone(),value_json(value))).collect::<Map<_,_>>()})
}
/// Encode the complete committed report, including original retry identities.
pub fn encode_report(report: &TxReport) -> String {
    report_json(report).to_string()
}
/// Encode a flat projection; unknown entities have a null entity and no fields.
pub fn encode_entity(entity: Option<&Entity>) -> String {
    match entity {
        None => json!({"version":1,"entity":null}).to_string(),
        Some(entity) => json!({"version":1,"entity":{"id":entity.id.to_string()},"attributes":entity.attributes.iter().map(|(name,value)| (name.as_str().to_owned(),match value { PulledValue::One(value) => value_json(value),PulledValue::Many(values) => json!(values.iter().map(value_json).collect::<Vec<_>>()) })).collect::<Map<_,_>>()}).to_string(),
    }
}
/// Encode a lookup result: the matching entity, or null.
pub fn encode_lookup(entity: Option<&crate::EntityId>) -> String {
    json!({"version":1,"entity":entity.map(|id| json!({"id":id.to_string()}))}).to_string()
}
/// Encode historically visible definitions.
pub fn encode_schema(schema: &Schema) -> String {
    json!({"version":1,"schema":schema.attributes().map(definition_json).collect::<Vec<_>>()} )
        .to_string()
}
/// Encode effective typed facts in the supplied owned order.
pub fn encode_facts(facts: &[Fact]) -> String {
    json!({"version":1,"facts":facts.iter().map(fact_json).collect::<Vec<_>>()} ).to_string()
}
/// Encode events with decimal revision and ordinal strings.
pub fn encode_history(events: &[FactEvent]) -> String {
    json!({"version":1,"events":events.iter().map(|event| { let mut fact = fact_json(&event.fact); fact["op"] = json!(operation(event.operation)); fact["revision"] = json!(event.revision.to_string()); fact["ordinal"] = json!(event.ordinal.to_string()); fact }).collect::<Vec<_>>()} ).to_string()
}
/// Encode every retained record, including empty deltas and metadata.
pub fn encode_transactions(records: &[TxRecord]) -> String {
    json!({"version":1,"transactions":records.iter().map(|record| json!({"revision":record.revision().to_string(),"report":report_json(&record.report)})).collect::<Vec<_>>()} ).to_string()
}

fn ty(ty: ValueType) -> &'static str {
    match ty {
        ValueType::Bool => "bool",
        ValueType::I64 => "i64",
        ValueType::F64 => "f64",
        ValueType::String => "string",
        ValueType::Bytes => "bytes",
        ValueType::Ref => "ref",
    }
}
fn code(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::InvalidRequest => "InvalidRequest",
        ErrorCode::UnknownAttribute => "UnknownAttribute",
        ErrorCode::TypeMismatch => "TypeMismatch",
        ErrorCode::UnknownEntity => "UnknownEntity",
        ErrorCode::ConflictingBatch => "ConflictingBatch",
        ErrorCode::UniqueViolation => "UniqueViolation",
        ErrorCode::PreconditionFailed => "PreconditionFailed",
        ErrorCode::HeadMismatch => "HeadMismatch",
        ErrorCode::IdempotencyConflict => "IdempotencyConflict",
        ErrorCode::InvalidSnapshot => "InvalidSnapshot",
        ErrorCode::Busy => "Busy",
        ErrorCode::Storage => "Storage",
        ErrorCode::UnsupportedFormat => "UnsupportedFormat",
    }
}
/// Encode stable error classification and structured context, without exposing
/// private Rust source-error internals. CLI diagnostics need not be parsed.
pub fn encode_error(error: &Error) -> String {
    let context = match error.context() {
        ErrorContext::Storage { operation } => json!({"operation":operation}),
        ErrorContext::HeadMismatch { expected, actual } => {
            json!({"expected":snapshot_json(expected),"actual":snapshot_json(actual)})
        }
        ErrorContext::Precondition {
            entity,
            attribute,
            expected,
            actual,
        } => {
            json!({"entity":{"id":entity.to_string()},"attr":attribute.as_str(),"expected":expected.iter().map(value_json).collect::<Vec<_>>(),"actual":actual.iter().map(value_json).collect::<Vec<_>>()})
        }
        ErrorContext::UniqueOwnership {
            attribute,
            value,
            owners,
        } => {
            json!({"attr":attribute.as_str(),"value":value_json(value),"owners":owners.iter().map(|id| id.to_string()).collect::<Vec<_>>()})
        }
        ErrorContext::Fact { fact } => fact_json(fact),
        ErrorContext::EntityAttribute { entity, attribute } => {
            json!({"entity":{"id":entity.to_string()},"attr":attribute.as_str()})
        }
        ErrorContext::Attribute { attribute } => json!({"attr":attribute.as_str()}),
        ErrorContext::Entity { entity } => json!({"entity":{"id":entity.to_string()}}),
        ErrorContext::InvalidInput { field } => json!({"field":field}),
        ErrorContext::TypeMismatch { expected, actual } => {
            json!({"expected":ty(*expected),"actual":ty(*actual)})
        }
    };
    let mut result = json!({"version":1,"error":{"code":code(error.code()),"message":error.to_string(),"context":context}});
    if let Some(outcome) = error.commit_outcome() {
        result["error"]["commit_outcome"] = json!(match outcome {
            CommitOutcome::RolledBack => "RolledBack",
            CommitOutcome::Unknown => "Unknown",
        });
    }
    result.to_string()
}
