//! Bounded decoder for retained canonicalization version 1.

use std::collections::BTreeMap;

use crate::{
    AttributeDefinition, Cardinality, Change, EntityTerm, Error, ErrorCode, ErrorContext,
    Expectation, Operation, Result, TempId, Tx, Uniqueness, Value, ValueTerm, ValueType,
    store::corrupt,
};

struct Reader<'a> {
    remaining: &'a [u8],
}
impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        if count > self.remaining.len() {
            return Err(corrupt("truncated canonical request"));
        }
        let (bytes, rest) = self.remaining.split_at(count);
        self.remaining = rest;
        Ok(bytes)
    }
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn eight(&mut self) -> Result<[u8; 8]> {
        Ok(self.take(8)?.try_into().expect("eight bytes"))
    }
    fn count(&mut self) -> Result<usize> {
        let count = usize::try_from(u64::from_be_bytes(self.eight()?))
            .map_err(|_| corrupt("canonical length overflow"))?;
        if count > self.remaining.len() {
            return Err(corrupt("canonical count exceeds remaining bytes"));
        }
        Ok(count)
    }
    fn bytes(&mut self) -> Result<&'a [u8]> {
        let count = self.count()?;
        self.take(count)
    }
    fn text(&mut self) -> Result<&'a str> {
        std::str::from_utf8(self.bytes()?).map_err(|_| corrupt("canonical text is not UTF-8"))
    }
    fn list<T>(&mut self, mut read: impl FnMut(&mut Self) -> Result<T>) -> Result<Vec<T>> {
        let count = self.count()?;
        (0..count).map(|_| read(self)).collect()
    }
    fn ty(&mut self) -> Result<ValueType> {
        match self.byte()? {
            0 => Ok(ValueType::Bool),
            1 => Ok(ValueType::I64),
            2 => Ok(ValueType::F64),
            3 => Ok(ValueType::String),
            4 => Ok(ValueType::Bytes),
            5 => Ok(ValueType::Ref),
            _ => Err(corrupt("invalid canonical type")),
        }
    }
    fn value(&mut self) -> Result<Value> {
        match self.ty()? {
            ValueType::Bool => match self.byte()? {
                0 => Ok(Value::Bool(false)),
                1 => Ok(Value::Bool(true)),
                _ => Err(corrupt("invalid canonical bool")),
            },
            ValueType::I64 => Ok(Value::I64(i64::from_be_bytes(self.eight()?))),
            ValueType::F64 => Value::f64(f64::from_bits(u64::from_be_bytes(self.eight()?)))
                .map_err(|error| corrupt(error.to_string())),
            ValueType::String => Ok(Value::String(self.text()?.into())),
            ValueType::Bytes => Ok(Value::Bytes(self.bytes()?.into())),
            ValueType::Ref => Ok(Value::Ref(
                self.text()?
                    .parse()
                    .map_err(|error: Error| corrupt(error.to_string()))?,
            )),
        }
    }
    fn entity(&mut self) -> Result<EntityTerm> {
        match self.byte()? {
            0 => Ok(EntityTerm::Id(
                self.text()?
                    .parse()
                    .map_err(|error: Error| corrupt(error.to_string()))?,
            )),
            1 => Ok(EntityTerm::Temp(
                TempId::new(self.text()?).map_err(|error| corrupt(error.to_string()))?,
            )),
            _ => Err(corrupt("invalid canonical entity term")),
        }
    }
    fn term(&mut self) -> Result<ValueTerm> {
        match self.byte()? {
            0 => Ok(ValueTerm::Value(self.value()?)),
            1 => Ok(ValueTerm::TempRef(
                TempId::new(self.text()?).map_err(|error| corrupt(error.to_string()))?,
            )),
            _ => Err(corrupt("invalid canonical value term")),
        }
    }
}

pub(crate) fn decode_request(version: i64, bytes: &[u8]) -> Result<Tx> {
    if version != 1 {
        return Err(Error::new(
            ErrorCode::UnsupportedFormat,
            ErrorContext::InvalidInput {
                field: "canonical encoding",
            },
            "unsupported retained canonicalization version",
        ));
    }
    let mut reader = Reader { remaining: bytes };
    if reader.take(8)? != b"FLC1\0\0\0\x01" {
        return Err(corrupt("canonical header/request version mismatch"));
    }
    let schema = reader.list(|reader| {
        let name = reader
            .text()?
            .parse()
            .map_err(|error: Error| corrupt(error.to_string()))?;
        let ty = reader.ty()?;
        let cardinality = match reader.byte()? {
            0 => Cardinality::One,
            1 => Cardinality::Many,
            _ => return Err(corrupt("invalid canonical cardinality")),
        };
        let uniqueness = match reader.byte()? {
            0 => Uniqueness::None,
            1 => Uniqueness::UniqueOne,
            _ => return Err(corrupt("invalid canonical uniqueness")),
        };
        AttributeDefinition::new(name, ty, cardinality, uniqueness)
            .map_err(|error| corrupt(error.to_string()))
    })?;
    let new_entities = reader
        .list(|reader| TempId::new(reader.text()?).map_err(|error| corrupt(error.to_string())))?;
    let changes = reader.list(|reader| {
        let operation = match reader.byte()? {
            0 => Operation::Retract,
            1 => Operation::Assert,
            _ => return Err(corrupt("invalid canonical operation")),
        };
        Ok(Change {
            operation,
            entity: reader.entity()?,
            attribute: reader
                .text()?
                .parse()
                .map_err(|error: Error| corrupt(error.to_string()))?,
            value: reader.term()?,
        })
    })?;
    let expectations = reader.list(|reader| {
        Ok(Expectation {
            entity: reader
                .text()?
                .parse()
                .map_err(|error: Error| corrupt(error.to_string()))?,
            attribute: reader
                .text()?
                .parse()
                .map_err(|error: Error| corrupt(error.to_string()))?,
            values: reader.list(Reader::value)?,
        })
    })?;
    let expected_head = match reader.byte()? {
        0 => None,
        1 => Some(
            reader
                .text()?
                .parse()
                .map_err(|error: Error| corrupt(error.to_string()))?,
        ),
        _ => return Err(corrupt("invalid canonical head marker")),
    };
    let entries = reader.list(|reader| Ok((reader.text()?.to_owned(), reader.value()?)))?;
    let mut metadata = BTreeMap::new();
    for (key, value) in entries {
        if metadata.insert(key, value).is_some() {
            return Err(corrupt("duplicate canonical metadata key"));
        }
    }
    let request = Tx {
        schema,
        new_entities,
        changes,
        expectations,
        metadata,
        expected_head,
        idempotency_key: None,
    }
    .validate()
    .map_err(|error| corrupt(error.to_string()))?;
    if !reader.remaining.is_empty() || request.canonical_request().bytes() != bytes {
        return Err(corrupt("noncanonical retained request"));
    }
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    #[test]
    fn retained_negative_zero_nonfinite_floats_and_oversized_counts_fail() {
        let original = Tx::builder()
            .metadata("f", Value::f64(0.0).unwrap())
            .build()
            .unwrap()
            .canonical_request();
        for bits in [
            (-0.0_f64).to_bits(),
            f64::NAN.to_bits(),
            f64::INFINITY.to_bits(),
        ] {
            let mut bytes = original.bytes().to_vec();
            let start = bytes.len() - 8;
            bytes[start..].copy_from_slice(&bits.to_be_bytes());
            assert_eq!(
                decode_request(1, &bytes).unwrap_err().code(),
                ErrorCode::Storage
            );
        }
        let mut bytes = original.bytes().to_vec();
        bytes[8..16].copy_from_slice(&u64::MAX.to_be_bytes());
        assert_eq!(
            decode_request(1, &bytes).unwrap_err().code(),
            ErrorCode::Storage
        );
    }
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1024))]
        #[test]
        fn canonical_decode_roundtrips_and_rejects_every_truncation(value in any::<i64>(), bits in any::<u64>(), bytes in proptest::collection::vec(any::<u8>(),0..64)) {
            let mut builder = Tx::builder().metadata("i",Value::I64(value)).metadata("bytes",Value::Bytes(bytes));
            if let Ok(value) = Value::f64(f64::from_bits(bits)) { builder = builder.metadata("f",value); }
            let tx = builder.build().unwrap(); let canonical = tx.canonical_request();
            prop_assert_eq!(decode_request(1,canonical.bytes()).unwrap(),tx);
            for end in 0..canonical.bytes().len() { prop_assert!(decode_request(1,&canonical.bytes()[..end]).is_err()); }
        }
        #[test]
        fn arbitrary_retained_canonical_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(),0..256)) {
            let _ = decode_request(1,&bytes);
        }
    }
}
