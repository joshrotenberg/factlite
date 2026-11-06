//! Version-one internal storage codecs, distinct from the public JSON contract.

use crate::store::corrupt;
use crate::{
    AttributeDefinition, Cardinality, EntityId, Fact, FactDelta, Operation, Result, SnapshotId,
    TempId, TxReport, Uniqueness, Value, ValueType,
};
use std::collections::BTreeMap;

const SIGN: u64 = 1 << 63;

pub(crate) fn type_tag(ty: ValueType) -> u8 {
    match ty {
        ValueType::Bool => 0,
        ValueType::I64 => 1,
        ValueType::F64 => 2,
        ValueType::String => 3,
        ValueType::Bytes => 4,
        ValueType::Ref => 5,
        ValueType::Keyword => 6,
        ValueType::Inst => 7,
    }
}
fn value_type(tag: u8) -> Result<ValueType> {
    match tag {
        0 => Ok(ValueType::Bool),
        1 => Ok(ValueType::I64),
        2 => Ok(ValueType::F64),
        3 => Ok(ValueType::String),
        4 => Ok(ValueType::Bytes),
        5 => Ok(ValueType::Ref),
        6 => Ok(ValueType::Keyword),
        7 => Ok(ValueType::Inst),
        _ => Err(corrupt("invalid stored value tag")),
    }
}

// Per-type order-preserving BLOBs; tags remain explicit and comparisons never coerce.
pub(crate) fn encode_value(value: &Value) -> Vec<u8> {
    match value {
        Value::Bool(value) => vec![u8::from(*value)],
        Value::I64(value) => ((*value as u64) ^ SIGN).to_be_bytes().to_vec(),
        Value::F64(value) => {
            let bits = value.get().to_bits();
            let key = if bits & SIGN != 0 { !bits } else { bits ^ SIGN };
            key.to_be_bytes().to_vec()
        }
        Value::String(value) => value.as_bytes().to_vec(),
        Value::Bytes(value) => value.clone(),
        Value::Ref(value) => value.to_string().into_bytes(),
        Value::Keyword(value) => value.as_str().as_bytes().to_vec(),
        // Instants order like integers.
        Value::Inst(value) => ((*value as u64) ^ SIGN).to_be_bytes().to_vec(),
    }
}

pub(crate) fn decode_value(tag: u8, bytes: &[u8]) -> Result<Value> {
    let eight = || -> Result<u64> {
        Ok(u64::from_be_bytes(bytes.try_into().map_err(|_| {
            corrupt("numeric value must contain eight bytes")
        })?))
    };
    let text = || -> Result<&str> {
        std::str::from_utf8(bytes).map_err(|_| corrupt("invalid stored UTF-8"))
    };
    let value = match value_type(tag)? {
        ValueType::Bool => match bytes {
            [0] => Value::Bool(false),
            [1] => Value::Bool(true),
            _ => return Err(corrupt("invalid stored boolean")),
        },
        ValueType::I64 => Value::I64((eight()? ^ SIGN) as i64),
        ValueType::F64 => {
            let key = eight()?;
            let bits = if key & SIGN != 0 { key ^ SIGN } else { !key };
            Value::f64(f64::from_bits(bits)).map_err(|err| corrupt(err.to_string()))?
        }
        ValueType::String => Value::String(text()?.to_owned()),
        ValueType::Bytes => Value::Bytes(bytes.to_vec()),
        ValueType::Ref => Value::Ref(
            text()?
                .parse()
                .map_err(|err: crate::Error| corrupt(err.to_string()))?,
        ),
        ValueType::Keyword => Value::keyword(text()?).map_err(|err| corrupt(err.to_string()))?,
        ValueType::Inst => Value::Inst((eight()? ^ SIGN) as i64),
    };
    if encode_value(&value) != bytes {
        return Err(corrupt("noncanonical stored value"));
    }
    Ok(value)
}

#[derive(Default)]
struct Writer(Vec<u8>);
impl Writer {
    fn byte(&mut self, value: u8) {
        self.0.push(value);
    }
    fn count(&mut self, value: usize) {
        self.0.extend_from_slice(&(value as u64).to_be_bytes());
    }
    fn bytes(&mut self, value: &[u8]) {
        self.count(value.len());
        self.0.extend_from_slice(value);
    }
    fn text(&mut self, value: &str) {
        self.bytes(value.as_bytes());
    }
    fn value(&mut self, value: &Value) {
        self.byte(type_tag(value.value_type()));
        self.bytes(&encode_value(value));
    }
    fn definition(&mut self, definition: &AttributeDefinition) {
        self.text(definition.name().as_str());
        self.byte(type_tag(definition.value_type()));
        self.byte(match definition.cardinality() {
            Cardinality::One => 0,
            Cardinality::Many => 1,
        });
        self.byte(match definition.uniqueness() {
            Uniqueness::None => 0,
            Uniqueness::UniqueOne => 1,
        });
    }
}
struct Reader<'a> {
    remaining: &'a [u8],
}
impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        if count > self.remaining.len() {
            return Err(corrupt("truncated stored record"));
        }
        let (value, remaining) = self.remaining.split_at(count);
        self.remaining = remaining;
        Ok(value)
    }
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn count(&mut self) -> Result<usize> {
        let value = u64::from_be_bytes(self.take(8)?.try_into().expect("eight bytes"));
        let count =
            usize::try_from(value).map_err(|_| corrupt("stored count exceeds host range"))?;
        if count > self.remaining.len() {
            return Err(corrupt("stored count exceeds record bounds"));
        }
        Ok(count)
    }
    fn bytes(&mut self) -> Result<&'a [u8]> {
        let count = self.count()?;
        self.take(count)
    }
    fn text(&mut self) -> Result<&'a str> {
        std::str::from_utf8(self.bytes()?).map_err(|_| corrupt("invalid stored UTF-8"))
    }
    fn entity(&mut self) -> Result<EntityId> {
        self.text()?
            .parse()
            .map_err(|err: crate::Error| corrupt(err.to_string()))
    }
    fn snapshot(&mut self) -> Result<SnapshotId> {
        self.text()?
            .parse()
            .map_err(|err: crate::Error| corrupt(err.to_string()))
    }
    fn value(&mut self) -> Result<Value> {
        let tag = self.byte()?;
        decode_value(tag, self.bytes()?)
    }
    fn definition(&mut self) -> Result<AttributeDefinition> {
        let name = self
            .text()?
            .parse()
            .map_err(|err: crate::Error| corrupt(err.to_string()))?;
        let ty = value_type(self.byte()?)?;
        let cardinality = match self.byte()? {
            0 => Cardinality::One,
            1 => Cardinality::Many,
            _ => return Err(corrupt("invalid stored cardinality")),
        };
        let uniqueness = match self.byte()? {
            0 => Uniqueness::None,
            1 => Uniqueness::UniqueOne,
            _ => return Err(corrupt("invalid stored uniqueness")),
        };
        AttributeDefinition::new(name, ty, cardinality, uniqueness)
            .map_err(|err| corrupt(err.to_string()))
    }
    fn done(&self) -> Result<()> {
        if self.remaining.is_empty() {
            Ok(())
        } else {
            Err(corrupt("trailing stored bytes"))
        }
    }
}

pub(crate) fn encode_definition(definition: &AttributeDefinition) -> Vec<u8> {
    let mut writer = Writer::default();
    writer.definition(definition);
    writer.0
}
pub(crate) fn decode_definition(bytes: &[u8]) -> Result<AttributeDefinition> {
    let mut reader = Reader { remaining: bytes };
    let definition = reader.definition()?;
    reader.done()?;
    Ok(definition)
}

pub(crate) fn encode_report(report: &TxReport) -> Vec<u8> {
    let mut writer = Writer(b"FLR1".to_vec());
    writer.text(&report.before.to_string());
    writer.text(&report.after.to_string());
    writer.count(report.tempids.len());
    for (label, id) in &report.tempids {
        writer.text(label.as_str());
        writer.text(&id.to_string());
    }
    writer.count(report.created_entities.len());
    for id in &report.created_entities {
        writer.text(&id.to_string());
    }
    writer.count(report.schema_added.len());
    for definition in &report.schema_added {
        writer.definition(definition);
    }
    writer.count(report.effective_delta.len());
    for delta in &report.effective_delta {
        writer.byte(match delta.operation {
            Operation::Retract => 0,
            Operation::Assert => 1,
        });
        writer.text(&delta.fact.entity.to_string());
        writer.text(delta.fact.attribute.as_str());
        writer.value(&delta.fact.value);
    }
    writer.count(report.metadata.len());
    for (key, value) in &report.metadata {
        writer.text(key);
        writer.value(value);
    }
    writer.0
}

pub(crate) fn decode_report(bytes: &[u8]) -> Result<TxReport> {
    let mut reader = Reader { remaining: bytes };
    if reader.take(4)? != b"FLR1" {
        return Err(corrupt("unsupported stored report magic"));
    }
    let before = reader.snapshot()?;
    let after = reader.snapshot()?;
    let mut tempids = BTreeMap::new();
    for _ in 0..reader.count()? {
        let label = TempId::new(reader.text()?).map_err(|err| corrupt(err.to_string()))?;
        if tempids.insert(label, reader.entity()?).is_some() {
            return Err(corrupt("duplicate stored temporary label"));
        }
    }
    let mut created_entities = Vec::new();
    for _ in 0..reader.count()? {
        created_entities.push(reader.entity()?);
    }
    let mut schema_added = Vec::new();
    for _ in 0..reader.count()? {
        schema_added.push(reader.definition()?);
    }
    let mut effective_delta = Vec::new();
    for _ in 0..reader.count()? {
        let operation = match reader.byte()? {
            0 => Operation::Retract,
            1 => Operation::Assert,
            _ => return Err(corrupt("invalid stored operation")),
        };
        let entity = reader.entity()?;
        let attribute = reader
            .text()?
            .parse()
            .map_err(|err: crate::Error| corrupt(err.to_string()))?;
        let value = reader.value()?;
        effective_delta.push(FactDelta {
            operation,
            fact: Fact {
                entity,
                attribute,
                value,
            },
        });
    }
    let mut metadata = BTreeMap::new();
    for _ in 0..reader.count()? {
        let key = reader.text()?.to_owned();
        let value = reader.value()?;
        if matches!(value, Value::Ref(_)) {
            return Err(corrupt("stored metadata contains a reference"));
        }
        if metadata.insert(key, value).is_some() {
            return Err(corrupt("duplicate stored metadata key"));
        }
    }
    reader.done()?;
    let report = TxReport {
        before,
        after,
        tempids,
        created_entities,
        schema_added,
        effective_delta,
        metadata,
    };
    if encode_report(&report) != bytes {
        return Err(corrupt("noncanonical stored report"));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    fn values() -> impl Strategy<Value = Value> {
        prop_oneof![
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(Value::I64),
            any::<u64>()
                .prop_filter("finite float", |bits| f64::from_bits(*bits).is_finite())
                .prop_map(|bits| Value::f64(f64::from_bits(bits)).unwrap()),
            any::<String>().prop_map(Value::String),
            proptest::collection::vec(any::<u8>(), 0..256).prop_map(Value::Bytes),
            "([a-z0-9.*+!_?<>=雪-]{1,6}/)?[a-z0-9.*+!_?<>=雪-]{1,6}"
                .prop_map(|text| Value::keyword(text).unwrap()),
            any::<i64>().prop_map(Value::Inst),
            (any::<u128>(), 1_u64..=u64::MAX).prop_map(|(store, allocation)| Value::Ref(
                format!("e1.{store:032x}.{allocation:016x}")
                    .parse()
                    .unwrap()
            ))
        ]
    }
    #[test]
    fn malformed_stored_values_and_records_fail() {
        assert!(decode_value(0, &[2]).is_err());
        assert!(decode_value(1, &[]).is_err());
        assert!(decode_value(3, &[255]).is_err());
        assert!(decode_value(6, &[]).is_err());
        // Negative-zero sort encoding, infinity, and NaN must not become valid data.
        for bits in [
            (-0.0_f64).to_bits(),
            f64::INFINITY.to_bits(),
            f64::NAN.to_bits(),
        ] {
            let key = if bits & SIGN != 0 { !bits } else { bits ^ SIGN };
            assert!(decode_value(2, &key.to_be_bytes()).is_err());
        }
        assert!(decode_report(b"FLR1\xff\xff\xff\xff\xff\xff\xff\xff").is_err());
    }
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1024))]
        #[test]
        fn every_value_kind_roundtrips(value in values()){prop_assert_eq!(decode_value(type_tag(value.value_type()),&encode_value(&value)).unwrap(),value);}
        #[test]
        fn scalar_blob_order_matches_typed_numeric_order(a in any::<i64>(),b in any::<i64>(),x in any::<f64>().prop_filter("finite",|v|v.is_finite()),y in any::<f64>().prop_filter("finite",|v|v.is_finite())){
            prop_assert_eq!(encode_value(&Value::I64(a)).cmp(&encode_value(&Value::I64(b))),a.cmp(&b));
            let x=Value::f64(x).unwrap();let y=Value::f64(y).unwrap();prop_assert_eq!(encode_value(&x).cmp(&encode_value(&y)),x.compare(&y).unwrap());
            prop_assert_eq!(encode_value(&Value::Inst(a)).cmp(&encode_value(&Value::Inst(b))),a.cmp(&b));
        }
        #[test]
        fn reports_roundtrip_exact_values(value in values(), cut in any::<usize>()){
            let label=TempId::new("e").unwrap();let definition=AttributeDefinition::new("x/value".parse().unwrap(),value.value_type(),Cardinality::One,Uniqueness::None).unwrap();
            // This codec test validates representation; replay separately validates membership.
            let id:EntityId="e1.00000000000000000000000000000001.0000000000000001".parse().unwrap();
            let mut report=TxReport {before:"d1.00000000000000000000000000000001.00000000000000000000000000000002.0".parse().unwrap(),after:"d1.00000000000000000000000000000001.00000000000000000000000000000002.1".parse().unwrap(),tempids:BTreeMap::from([(label,id.clone())]),created_entities:vec![id.clone()],schema_added:vec![definition],effective_delta:vec![FactDelta {operation:Operation::Assert,fact:Fact {entity:id,attribute:"x/value".parse().unwrap(),value:value.clone()}}],metadata:BTreeMap::new()};
            if !matches!(value,Value::Ref(_)){report.metadata.insert("typed".into(),value);}
            let encoded=encode_report(&report);
            prop_assert!(decode_report(&encoded[..cut%encoded.len()]).is_err());
            prop_assert_eq!(decode_report(&encoded).unwrap(),report);
        }
        #[test]
        fn arbitrary_storage_decoders_never_panic(bytes in proptest::collection::vec(any::<u8>(),0..512),tag in any::<u8>()) {let _=decode_report(&bytes);let _=decode_definition(&bytes);let _=decode_value(tag,&bytes);}
    }
}
