use std::{
    cmp::Ordering,
    hash::{Hash, Hasher},
};

use crate::{EntityId, Error, ErrorCode, ErrorContext, Result};

/// The exact stored type of a fact value; numeric types never coerce.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ValueType {
    /// Boolean.
    Bool,
    /// Signed 64-bit integer.
    I64,
    /// Finite IEEE 754 binary64.
    F64,
    /// Exact UTF-8 string.
    String,
    /// Byte sequence.
    Bytes,
    /// Store-bound entity reference.
    Ref,
    /// Symbolic name, distinct from strings: `name` or `namespace/name`.
    Keyword,
    /// UTC instant as signed milliseconds since the Unix epoch.
    Inst,
}

/// A finite binary64 value whose negative zero is normalized to positive zero.
#[derive(Clone, Copy, Debug)]
pub struct FiniteF64(f64);

impl FiniteF64 {
    /// Rejects NaN/infinities and normalizes zero before storage or comparison.
    pub fn new(value: f64) -> Result<Self> {
        if !value.is_finite() {
            return Err(Error::invalid("f64", "float must be finite"));
        }
        Ok(Self(if value == 0.0 { 0.0 } else { value }))
    }
    /// Returns the normalized finite value.
    pub fn get(self) -> f64 {
        self.0
    }
}

impl PartialEq for FiniteF64 {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}
impl Eq for FiniteF64 {}
impl Hash for FiniteF64 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}
impl Ord for FiniteF64 {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}
impl PartialOrd for FiniteF64 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A symbolic name: `name` or `namespace/name`, without a leading colon. Each
/// part is nonempty and contains no `/`, whitespace or control character, so a
/// keyword always reads back as the same Clojure keyword. Ordering is bytewise
/// over the text, as for strings, but a keyword never equals a string.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Keyword(String);

impl Keyword {
    /// Validates the keyword text, without a leading colon.
    pub fn new(text: impl Into<String>) -> Result<Self> {
        let text = text.into();
        let valid_part = |part: &str| {
            !part.is_empty()
                && !part
                    .chars()
                    .any(|c| c == '/' || c.is_whitespace() || c.is_control())
        };
        let valid = match text.split_once('/') {
            None => valid_part(&text),
            Some((namespace, name)) => valid_part(namespace) && valid_part(name),
        };
        if !valid || text.starts_with(':') {
            return Err(Error::invalid(
                "keyword",
                "keywords are name or namespace/name, each nonempty without '/', ':' prefix, whitespace or control characters",
            ));
        }
        Ok(Self(text))
    }
    /// The full text, such as `status/done`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// The namespace, if any.
    pub fn namespace(&self) -> Option<&str> {
        self.0.split_once('/').map(|(namespace, _)| namespace)
    }
    /// The name after any namespace.
    pub fn name(&self) -> &str {
        self.0.split_once('/').map_or(&self.0, |(_, name)| name)
    }
}

/// A typed fact value. Absence is represented by no fact, never a null value.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Value {
    /// Boolean value.
    Bool(bool),
    /// Full-range signed integer.
    I64(i64),
    /// Validated finite float.
    F64(FiniteF64),
    /// Exact string with no normalization or case folding.
    String(String),
    /// Exact byte sequence.
    Bytes(Vec<u8>),
    /// Entity reference; catalog membership is validated by transactions.
    Ref(EntityId),
    /// Validated keyword; never equal to a string with the same text.
    Keyword(Keyword),
    /// UTC instant in milliseconds since the Unix epoch, over the full i64
    /// range; never equal to an I64 with the same number.
    Inst(i64),
}

impl Value {
    // Deterministic report/set encoding only; public comparisons still reject
    // mixed types. Within a validated attribute all values have one type.
    pub(crate) fn canonical_cmp(&self, other: &Self) -> Ordering {
        self.value_type()
            .cmp(&other.value_type())
            .then_with(|| self.compare(other).expect("matching type tags"))
    }

    /// Constructs a validated keyword value.
    pub fn keyword(text: impl Into<String>) -> Result<Self> {
        Keyword::new(text).map(Self::Keyword)
    }
    /// Constructs a validated float value.
    pub fn f64(value: f64) -> Result<Self> {
        FiniteF64::new(value).map(Self::F64)
    }
    /// Returns the exact type tag.
    pub fn value_type(&self) -> ValueType {
        match self {
            Self::Bool(_) => ValueType::Bool,
            Self::I64(_) => ValueType::I64,
            Self::F64(_) => ValueType::F64,
            Self::String(_) => ValueType::String,
            Self::Bytes(_) => ValueType::Bytes,
            Self::Ref(_) => ValueType::Ref,
            Self::Keyword(_) => ValueType::Keyword,
            Self::Inst(_) => ValueType::Inst,
        }
    }
    /// Compares within one type; cross-type ordering returns `TypeMismatch`.
    pub fn compare(&self, other: &Self) -> Result<Ordering> {
        match (self, other) {
            (Self::Bool(a), Self::Bool(b)) => Ok(a.cmp(b)),
            (Self::I64(a), Self::I64(b)) => Ok(a.cmp(b)),
            (Self::F64(a), Self::F64(b)) => Ok(a.cmp(b)),
            (Self::String(a), Self::String(b)) => Ok(a.as_bytes().cmp(b.as_bytes())),
            (Self::Bytes(a), Self::Bytes(b)) => Ok(a.cmp(b)),
            (Self::Ref(a), Self::Ref(b)) => Ok(a.cmp(b)),
            (Self::Keyword(a), Self::Keyword(b)) => {
                Ok(a.as_str().as_bytes().cmp(b.as_str().as_bytes()))
            }
            (Self::Inst(a), Self::Inst(b)) => Ok(a.cmp(b)),
            _ => Err(Error::new(
                ErrorCode::TypeMismatch,
                ErrorContext::TypeMismatch {
                    expected: self.value_type(),
                    actual: other.value_type(),
                },
                "cross-type ordering is undefined",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::{collections::HashSet, hash::DefaultHasher};

    fn hash(value: &Value) -> u64 {
        let mut h = DefaultHasher::new();
        value.hash(&mut h);
        h.finish()
    }

    #[test]
    fn typed_equality_and_zero_hashing() {
        let distinct = HashSet::from([
            Value::Bool(true),
            Value::I64(1),
            Value::f64(1.0).unwrap(),
            Value::String("1".into()),
            Value::Bytes(vec![1]),
        ]);
        assert_eq!(distinct.len(), 5);
        let negative = Value::f64(-0.0).unwrap();
        let positive = Value::f64(0.0).unwrap();
        assert_eq!(negative, positive);
        assert_eq!(hash(&negative), hash(&positive));
        assert_eq!(FiniteF64::new(-0.0).unwrap().get().to_bits(), 0);
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(Value::f64(value).is_err());
        }
        for value in [f64::MIN, f64::MAX, f64::MIN_POSITIVE, f64::from_bits(1)] {
            assert_eq!(FiniteF64::new(value).unwrap().get(), value);
        }
        assert_ne!(Value::String("é".into()), Value::String("e\u{301}".into()));
        assert_ne!(Value::String("Ada".into()), Value::String("ada".into()));
        assert_eq!(
            Value::I64(i64::MIN).compare(&Value::I64(i64::MAX)).unwrap(),
            Ordering::Less
        );
        assert_eq!(
            Value::I64(1)
                .compare(&Value::f64(1.0).unwrap())
                .unwrap_err()
                .code(),
            ErrorCode::TypeMismatch
        );
    }

    #[test]
    fn keywords_and_insts_have_their_own_types_and_order() {
        for valid in ["todo", "status/done", "a.b/c-d?", "雪/<=>", "x+*!_"] {
            assert_eq!(Keyword::new(valid).unwrap().as_str(), valid);
        }
        for invalid in ["", "/", "a/", "/a", "a/b/c", ":a", "a b", "a\tb", "a\u{0}"] {
            assert!(Keyword::new(invalid).is_err(), "{invalid:?}");
        }
        let done = Keyword::new("status/done").unwrap();
        assert_eq!((done.namespace(), done.name()), (Some("status"), "done"));
        let plain = Keyword::new("todo").unwrap();
        assert_eq!((plain.namespace(), plain.name()), (None, "todo"));
        assert_ne!(Value::keyword("a").unwrap(), Value::String("a".into()));
        assert_ne!(Value::Inst(1), Value::I64(1));
        assert_eq!(
            Value::keyword("a")
                .unwrap()
                .compare(&Value::String("a".into()))
                .unwrap_err()
                .code(),
            ErrorCode::TypeMismatch
        );
        assert_eq!(
            Value::Inst(i64::MIN).compare(&Value::Inst(0)).unwrap(),
            Ordering::Less
        );
        assert_eq!(
            Value::keyword("a/b")
                .unwrap()
                .compare(&Value::keyword("a/c").unwrap())
                .unwrap(),
            Ordering::Less
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1024))]
        #[test]
        fn floats_validate_all_bit_patterns(bits in any::<u64>()) {
            let input = f64::from_bits(bits); let result = FiniteF64::new(input);
            prop_assert_eq!(result.is_ok(), input.is_finite());
            if let Ok(value) = result { prop_assert_eq!(value.get().to_bits(), if input == 0.0 { 0 } else { bits }); }
        }
        #[test]
        fn float_order_is_numerical(a in any::<f64>().prop_filter("finite", |x| x.is_finite()), b in any::<f64>().prop_filter("finite", |x| x.is_finite())) {
            let a = FiniteF64::new(a).unwrap(); let b = FiniteF64::new(b).unwrap();
            prop_assert_eq!(a.cmp(&b), a.get().partial_cmp(&b.get()).unwrap());
        }
        #[test]
        fn integer_order_is_full_range(a in any::<i64>(), b in any::<i64>()) {
            prop_assert_eq!(Value::I64(a).compare(&Value::I64(b)).unwrap(), a.cmp(&b));
        }
        #[test]
        fn exact_bytes_and_strings(a in any::<String>(), b in any::<String>(), bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
            prop_assert_eq!(Value::String(a.clone()).compare(&Value::String(b.clone())).unwrap(), a.as_bytes().cmp(b.as_bytes()));
            prop_assert_eq!(Value::Bytes(bytes.clone()), Value::Bytes(bytes));
        }
    }
}
