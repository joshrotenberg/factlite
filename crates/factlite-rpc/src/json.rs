//! One level of strict JSON: an object's members or an array's elements,
//! each kept as the text the client sent.
//!
//! Duplicate keys are refused at the level parsed. Nested text is checked for
//! syntax only and handed on unchanged, so the wire decoders apply their own
//! strictness to exactly what arrived.

use std::{collections::HashSet, fmt};

use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::value::RawValue;

/// Why one level of JSON could not be taken apart.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Malformed {
    /// Not JSON, or trailing text after the value.
    Syntax(String),
    /// Valid JSON whose object repeats a key.
    Duplicate(String),
}

/// One JSON value, opened one level.
pub(crate) enum Top {
    Object(Members),
    Array(Vec<Box<RawValue>>),
    /// A string, number, boolean or null.
    Scalar,
}

/// The members of one object, in the order sent, each as raw text.
#[derive(Debug, Default)]
pub(crate) struct Members(Vec<(String, Box<RawValue>)>);

const DUPLICATE: &str = "duplicate object key: ";

impl<'de> Deserialize<'de> for Top {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TopVisitor;
        impl<'de> Visitor<'de> for TopVisitor {
            type Value = Top;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON value")
            }
            fn visit_bool<E>(self, _: bool) -> Result<Top, E> {
                Ok(Top::Scalar)
            }
            fn visit_i64<E>(self, _: i64) -> Result<Top, E> {
                Ok(Top::Scalar)
            }
            fn visit_u64<E>(self, _: u64) -> Result<Top, E> {
                Ok(Top::Scalar)
            }
            fn visit_f64<E>(self, _: f64) -> Result<Top, E> {
                Ok(Top::Scalar)
            }
            fn visit_str<E>(self, _: &str) -> Result<Top, E> {
                Ok(Top::Scalar)
            }
            fn visit_unit<E>(self) -> Result<Top, E> {
                Ok(Top::Scalar)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Top, A::Error> {
                let mut elements = Vec::new();
                while let Some(element) = sequence.next_element::<Box<RawValue>>()? {
                    elements.push(element);
                }
                Ok(Top::Array(elements))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Top, A::Error> {
                let mut members: Vec<(String, Box<RawValue>)> = Vec::new();
                let mut seen = HashSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !seen.insert(key.clone()) {
                        return Err(serde::de::Error::custom(format!("{DUPLICATE}{key}")));
                    }
                    members.push((key, map.next_value()?));
                }
                Ok(Top::Object(Members(members)))
            }
        }
        deserializer.deserialize_any(TopVisitor)
    }
}

impl Top {
    /// Parses one complete JSON value. Trailing text is a syntax error.
    ///
    /// The whole text is checked as JSON first, without converting numbers,
    /// so a syntax error anywhere is reported as one even when the text also
    /// repeats a key, and a number too large for a machine type is still
    /// JSON. Only then is the top level opened and checked for repeats.
    pub(crate) fn parse(text: &str) -> Result<Self, Malformed> {
        let raw: &RawValue =
            serde_json::from_str(text).map_err(|error| Malformed::Syntax(error.to_string()))?;
        if !matches!(kind(raw), Kind::Object | Kind::Array) {
            return Ok(Top::Scalar);
        }
        serde_json::from_str::<Top>(raw.get()).map_err(|error| {
            // The only data error this parser raises is the duplicate key;
            // anything else is serde_json refusing a key it cannot decode.
            if error.classify() == serde_json::error::Category::Data {
                Malformed::Duplicate(strip_position(&error.to_string()))
            } else {
                Malformed::Syntax(error.to_string())
            }
        })
    }
}

/// serde_json appends " at line L column C"; a single-line protocol has no
/// use for the line, and the column of a duplicate key says little.
fn strip_position(message: &str) -> String {
    match message.rfind(" at line ") {
        Some(index) => message[..index].to_owned(),
        None => message.to_owned(),
    }
}

impl Members {
    /// Removes and returns a member's raw text.
    pub(crate) fn take(&mut self, key: &str) -> Option<Box<RawValue>> {
        let index = self.0.iter().position(|(name, _)| name == key)?;
        Some(self.0.remove(index).1)
    }
    /// The first member not yet taken, if any: an unknown member once every
    /// known one has been taken.
    pub(crate) fn remaining(&self) -> Option<&str> {
        self.0.first().map(|(name, _)| name.as_str())
    }
    /// Whether a member is present.
    pub(crate) fn contains(&self, key: &str) -> bool {
        self.0.iter().any(|(name, _)| name == key)
    }
    /// Adds a member. The caller keeps keys distinct.
    pub(crate) fn insert(&mut self, key: &str, value: Box<RawValue>) {
        debug_assert!(!self.contains(key));
        self.0.push((key.to_owned(), value));
    }
    /// The members not yet taken, in the order sent.
    pub(crate) fn into_vec(self) -> Vec<(String, Box<RawValue>)> {
        self.0
    }
}

/// The kind of a raw JSON value, from its first byte. Raw text never starts
/// with whitespace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Kind {
    Object,
    Array,
    String,
    Number,
    Bool,
    Null,
}

pub(crate) fn kind(raw: &RawValue) -> Kind {
    match raw.get().as_bytes().first() {
        Some(b'{') => Kind::Object,
        Some(b'[') => Kind::Array,
        Some(b'"') => Kind::String,
        Some(b't' | b'f') => Kind::Bool,
        Some(b'n') => Kind::Null,
        _ => Kind::Number,
    }
}

/// A JSON string's contents, or None for any other kind.
pub(crate) fn string(raw: &RawValue) -> Option<String> {
    (kind(raw) == Kind::String)
        .then(|| serde_json::from_str(raw.get()).ok())
        .flatten()
}

/// A JSON boolean, or None for any other kind.
pub(crate) fn boolean(raw: &RawValue) -> Option<bool> {
    match raw.get() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// A JSON integer that fits u64, written without sign, fraction or exponent.
/// Raw JSON number text never has a leading `+`, so this is exactly what
/// `u64` parsing accepts.
pub(crate) fn unsigned(raw: &RawValue) -> Option<u64> {
    raw.get().parse().ok()
}

/// Quotes text as a JSON string.
pub(crate) fn quote(text: &str) -> String {
    serde_json::Value::String(text.to_owned()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn members(text: &str) -> Members {
        match Top::parse(text).unwrap() {
            Top::Object(members) => members,
            _ => panic!("not an object: {text}"),
        }
    }

    #[test]
    fn members_keep_their_text_and_nested_duplicates_are_left_for_the_decoder() {
        let mut object = members(r#" { "a" : {"x":1,"x":2} , "b":[1, 2 ] ,"c": 1.50e3 } "#);
        assert_eq!(object.take("a").unwrap().get(), r#"{"x":1,"x":2}"#);
        assert_eq!(object.take("b").unwrap().get(), "[1, 2 ]");
        assert_eq!(object.take("c").unwrap().get(), "1.50e3");
        assert!(object.take("a").is_none());
        assert_eq!(object.remaining(), None);
    }

    #[test]
    fn a_repeated_key_at_the_parsed_level_is_named_even_when_escaped() {
        for text in [r#"{"a":1,"a":2}"#, r#"{"a":1,"\u0061":2}"#] {
            assert_eq!(
                Top::parse(text).err(),
                Some(Malformed::Duplicate("duplicate object key: a".into())),
                "{text}"
            );
        }
    }

    /// Duplicate detection must not grow with the square of the key count:
    /// one line of the default size can hold over a million keys. The
    /// quadratic form took about 20 s for 80,000 keys in a debug build; the
    /// bound here is loose, to absorb a slow runner.
    #[test]
    fn many_keys_are_checked_for_repeats_in_linear_time() {
        let keys = 200_000;
        let mut text = String::from("{");
        for index in 0..keys {
            text.push_str(&format!("\"k{index}\":0,"));
        }
        text.push_str("\"k0\":1}");
        let started = std::time::Instant::now();
        assert_eq!(
            Top::parse(&text).err(),
            Some(Malformed::Duplicate("duplicate object key: k0".into()))
        );
        let text = text.replace(",\"k0\":1}", "}");
        assert!(matches!(Top::parse(&text), Ok(Top::Object(_))));
        assert!(started.elapsed() < std::time::Duration::from_secs(15));
    }

    #[test]
    fn text_that_is_not_one_json_value_is_a_syntax_error_at_any_depth() {
        for text in [
            "",
            "{",
            "{} {}",
            r#"{"a":}"#,
            r#"{"a":[1,}"#,
            r#"{"a":{"b":tru}}"#,
            "{\"a\":\"line\nbreak\"}",
            "[1,2",
            "nul",
        ] {
            assert!(
                matches!(Top::parse(text), Err(Malformed::Syntax(_))),
                "{text}"
            );
        }
    }

    /// Nested text is checked for structure, not for what its strings
    /// decode to. A lone surrogate passes this level and is refused by
    /// whoever decodes the string.
    #[test]
    fn a_nested_string_that_cannot_be_decoded_is_left_for_its_decoder() {
        let mut object = members(r#"{"a":"\ud800","b":{"c":"\ud800"}}"#);
        let raw = object.take("a").unwrap();
        assert_eq!(kind(&raw), Kind::String);
        assert_eq!(string(&raw), None);
        assert_eq!(object.take("b").unwrap().get(), r#"{"c":"\ud800"}"#);
        assert!(matches!(
            Top::parse(r#"{"\ud800":1}"#),
            Err(Malformed::Syntax(_))
        ));
    }

    /// A syntax error is reported as one even after a repeated key, and a
    /// number JSON allows is a scalar however large it is.
    #[test]
    fn syntax_is_checked_before_repeats_and_numbers_are_not_converted() {
        for text in [
            r#"{"id":1,"id":}"#,
            r#"{"id":1,"id":2} trailing"#,
            r#"{"a":1,"a":[}"#,
        ] {
            assert!(
                matches!(Top::parse(text), Err(Malformed::Syntax(_))),
                "{text}"
            );
        }
        for text in ["1e400", "-1e400", "123456789012345678901234567890"] {
            assert!(matches!(Top::parse(text), Ok(Top::Scalar)), "{text}");
        }
        assert!(matches!(Top::parse("[1e400]"), Ok(Top::Array(_))));
        let mut object = members(r#"{"n":1e400}"#);
        assert_eq!(object.take("n").unwrap().get(), "1e400");
    }

    #[test]
    fn arrays_and_scalars_are_told_apart() {
        match Top::parse(r#"[{"a":1}, 2,"x"]"#).unwrap() {
            Top::Array(elements) => {
                let text: Vec<_> = elements.iter().map(|element| element.get()).collect();
                assert_eq!(text, [r#"{"a":1}"#, "2", r#""x""#]);
            }
            _ => panic!("expected an array"),
        }
        for text in ["null", "1", "-1.5", "true", r#""x""#] {
            assert!(matches!(Top::parse(text).unwrap(), Top::Scalar), "{text}");
        }
    }

    #[test]
    fn scalars_are_read_only_from_their_own_kind() {
        let mut object = members(
            r#"{"s":"a\nb","t":true,"f":false,"n":12,"big":18446744073709551616,"frac":1.0,"exp":1e3,"neg":-1,"null":null,"str":"12"}"#,
        );
        let mut take = |key: &str| object.take(key).unwrap();
        assert_eq!(string(&take("s")).as_deref(), Some("a\nb"));
        assert_eq!(boolean(&take("t")), Some(true));
        assert_eq!(boolean(&take("f")), Some(false));
        assert_eq!(unsigned(&take("n")), Some(12));
        for key in ["big", "frac", "exp", "neg", "null"] {
            let raw = take(key);
            assert_eq!(unsigned(&raw), None, "{key}");
            assert_eq!(string(&raw), None, "{key}");
            assert_eq!(boolean(&raw), None, "{key}");
        }
        let text = take("str");
        assert_eq!(unsigned(&text), None);
        assert_eq!(kind(&text), Kind::String);
    }
}
