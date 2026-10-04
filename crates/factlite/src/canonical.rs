//! Frozen canonicalization version 1, independent of JSON and report codecs.

use crate::{Cardinality, EntityTerm, Operation, Tx, Uniqueness, Value, ValueTerm};

/// Exact retry identity, containing canonicalization version and full bytes.
/// The retry key is excluded; no allocated temporary IDs or database state enter
/// this representation. Operation/declaration/expectation-entry order is retained.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalRequest {
    bytes: Vec<u8>,
}
impl CanonicalRequest {
    /// Canonicalization version, distinct from the submitted request version.
    pub const fn version(&self) -> u32 {
        1
    }
    /// Complete canonical evidence; a digest must never replace exact comparison.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

struct Writer(Vec<u8>);
impl Writer {
    fn byte(&mut self, byte: u8) {
        self.0.push(byte);
    }
    fn count(&mut self, count: usize) {
        self.0.extend_from_slice(&(count as u64).to_be_bytes());
    }
    fn bytes(&mut self, bytes: &[u8]) {
        self.count(bytes.len());
        self.0.extend_from_slice(bytes);
    }
    fn text(&mut self, text: &str) {
        self.bytes(text.as_bytes());
    }
    fn value(&mut self, value: &Value) {
        match value {
            Value::Bool(value) => {
                self.byte(0);
                self.byte(u8::from(*value));
            }
            Value::I64(value) => {
                self.byte(1);
                self.0.extend_from_slice(&value.to_be_bytes());
            }
            Value::F64(value) => {
                self.byte(2);
                self.0
                    .extend_from_slice(&value.get().to_bits().to_be_bytes());
            }
            Value::String(value) => {
                self.byte(3);
                self.text(value);
            }
            Value::Bytes(value) => {
                self.byte(4);
                self.bytes(value);
            }
            Value::Ref(id) => {
                self.byte(5);
                self.text(&id.to_string());
            }
        }
    }
    fn entity(&mut self, entity: &EntityTerm) {
        match entity {
            EntityTerm::Id(id) => {
                self.byte(0);
                self.text(&id.to_string());
            }
            EntityTerm::Temp(label) => {
                self.byte(1);
                self.text(label.as_str());
            }
        }
    }
    fn term(&mut self, term: &ValueTerm) {
        match term {
            ValueTerm::Value(value) => {
                self.byte(0);
                self.value(value);
            }
            ValueTerm::TempRef(label) => {
                self.byte(1);
                self.text(label.as_str());
            }
        }
    }
}

impl Tx {
    /// Compute exact retry identity from the submitted, structurally validated
    /// typed request. Equal results need not imply equal keys. Reordering a
    /// semantically equivalent batch or renaming temporary labels changes identity.
    pub fn canonical_request(&self) -> CanonicalRequest {
        let mut writer = Writer(b"FLC1".to_vec());
        writer.0.extend_from_slice(&self.version().to_be_bytes());
        writer.count(self.schema.len());
        for definition in &self.schema {
            writer.text(definition.name().as_str());
            writer.byte(match definition.value_type() {
                crate::ValueType::Bool => 0,
                crate::ValueType::I64 => 1,
                crate::ValueType::F64 => 2,
                crate::ValueType::String => 3,
                crate::ValueType::Bytes => 4,
                crate::ValueType::Ref => 5,
            });
            writer.byte(match definition.cardinality() {
                Cardinality::One => 0,
                Cardinality::Many => 1,
            });
            writer.byte(match definition.uniqueness() {
                Uniqueness::None => 0,
                Uniqueness::UniqueOne => 1,
            });
        }
        writer.count(self.new_entities.len());
        for label in &self.new_entities {
            writer.text(label.as_str());
        }
        writer.count(self.changes.len());
        for change in &self.changes {
            writer.byte(match change.operation {
                Operation::Retract => 0,
                Operation::Assert => 1,
            });
            writer.entity(&change.entity);
            writer.text(change.attribute.as_str());
            writer.term(&change.value);
        }
        writer.count(self.expectations.len());
        for expect in &self.expectations {
            writer.text(&expect.entity.to_string());
            writer.text(expect.attribute.as_str());
            // Both builder and wire decoder normalize this exact typed set.
            writer.count(expect.values.len());
            for value in &expect.values {
                writer.value(value);
            }
        }
        match &self.expected_head {
            None => writer.byte(0),
            Some(head) => {
                writer.byte(1);
                writer.text(&head.to_string());
            }
        }
        writer.count(self.metadata.len());
        for (key, value) in &self.metadata {
            writer.text(key);
            writer.value(value);
        }
        CanonicalRequest { bytes: writer.0 }
    }
}
