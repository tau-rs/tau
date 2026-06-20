//! JSON-shaped values used by manifest capability params and tool
//! args/results.
//!
//! `BTreeMap` (not `HashMap`) for deterministic iteration order — matters
//! for golden tests and stable wire format.
//!
//! ## Wire format for `Value::Bytes`
//!
//! `Value::Bytes(Vec<u8>)` serializes as a JSON string with the literal
//! prefix `"@bytes:"` followed by standard base64 encoding of the bytes.
//! This disambiguates `Bytes` from `Array(Vec<Value>)` on the wire — without
//! it, an empty array (`[]`) and empty bytes (`b""`) collapse to the same
//! representation under `serde(untagged)`-style dispatch, breaking
//! round-trips. Strings starting with `"@bytes:"` are reserved and rejected
//! at serialize time.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// A JSON-shaped value: nullable, scalar, or recursive.
///
/// # Example
///
/// ```
/// use tau_domain::Value;
/// use std::collections::BTreeMap;
///
/// let v = Value::Object({
///     let mut m = BTreeMap::new();
///     m.insert("paths".into(), Value::Array(vec![Value::String("/tmp".into())]));
///     m
/// });
/// assert_eq!(
///     v.as_object().unwrap().get("paths").unwrap().as_array().unwrap().len(),
///     1,
/// );
/// ```
///
/// # `Value::Bytes` wire format
///
/// ```
/// use tau_domain::Value;
/// # #[cfg(feature = "serde")] {
/// let original = Value::Bytes(b"hello".to_vec());
/// let json = serde_json::to_string(&original).unwrap();
/// assert_eq!(json, r#""@bytes:aGVsbG8=""#);
/// let back: Value = serde_json::from_str(&json).unwrap();
/// assert_eq!(original, back);
/// # }
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// JSON null.
    Null,
    /// Boolean.
    Bool(bool),
    /// Signed 64-bit integer.
    Integer(i64),
    /// IEEE-754 double-precision float.
    Float(f64),
    /// UTF-8 string.
    ///
    /// **Reserved prefix:** strings starting with the literal `"@bytes:"`
    /// are reserved for the [`Value::Bytes`] wire format and are rejected
    /// at serialize time with a typed error.
    String(String),
    /// Binary blob (image data, file contents, etc.).
    ///
    /// Serializes as a JSON string of the form `"@bytes:<base64>"`, where
    /// `<base64>` is the standard base64 encoding of the byte slice.
    Bytes(Vec<u8>),
    /// Ordered list of values.
    Array(Vec<Value>),
    /// Sorted-key map of values.
    Object(BTreeMap<String, Value>),
}

impl Value {
    /// Return the inner string if this is `Value::String`, else `None`.
    pub fn as_string(&self) -> Option<&str> {
        if let Value::String(s) = self {
            Some(s)
        } else {
            None
        }
    }
    /// Return the inner integer if this is `Value::Integer`, else `None`.
    pub fn as_integer(&self) -> Option<i64> {
        if let Value::Integer(i) = self {
            Some(*i)
        } else {
            None
        }
    }
    /// Return the inner float if this is `Value::Float`, else `None`.
    pub fn as_float(&self) -> Option<f64> {
        if let Value::Float(f) = self {
            Some(*f)
        } else {
            None
        }
    }
    /// Return the inner bool if this is `Value::Bool`, else `None`.
    pub fn as_bool(&self) -> Option<bool> {
        if let Value::Bool(b) = self {
            Some(*b)
        } else {
            None
        }
    }
    /// Return the inner bytes if this is `Value::Bytes`, else `None`.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        if let Value::Bytes(b) = self {
            Some(b)
        } else {
            None
        }
    }
    /// Return the inner array if this is `Value::Array`, else `None`.
    pub fn as_array(&self) -> Option<&[Value]> {
        if let Value::Array(a) = self {
            Some(a)
        } else {
            None
        }
    }
    /// Return the inner object if this is `Value::Object`, else `None`.
    pub fn as_object(&self) -> Option<&BTreeMap<String, Value>> {
        if let Value::Object(o) = self {
            Some(o)
        } else {
            None
        }
    }
    /// True if this is `Value::Null`.
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
}

#[cfg(feature = "serde")]
mod value_serde {
    //! Manual `Serialize` / `Deserialize` for [`Value`].
    //!
    //! See module-level docs on the reserved `@bytes:` prefix.

    use super::Value;
    use alloc::borrow::ToOwned;
    use alloc::collections::BTreeMap;
    use alloc::string::String;
    use alloc::vec::Vec;
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
    use core::fmt;
    use serde::de::{self, MapAccess, SeqAccess, Visitor};
    use serde::ser::{SerializeMap, SerializeSeq, Serializer};
    use serde::{Deserialize, Deserializer, Serialize};

    /// Reserved prefix marking a base64-encoded `Value::Bytes` payload on
    /// the wire.
    pub(super) const BYTES_PREFIX: &str = "@bytes:";

    impl Serialize for Value {
        fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
            match self {
                Value::Null => ser.serialize_unit(),
                Value::Bool(b) => ser.serialize_bool(*b),
                Value::Integer(i) => ser.serialize_i64(*i),
                Value::Float(f) => ser.serialize_f64(*f),
                Value::String(s) => {
                    if s.starts_with(BYTES_PREFIX) {
                        Err(serde::ser::Error::custom(
                            "string value cannot start with reserved '@bytes:' prefix",
                        ))
                    } else {
                        ser.serialize_str(s)
                    }
                }
                Value::Bytes(b) => {
                    let encoded = format!("{}{}", BYTES_PREFIX, BASE64.encode(b));
                    ser.serialize_str(&encoded)
                }
                Value::Array(items) => {
                    let mut seq = ser.serialize_seq(Some(items.len()))?;
                    for item in items {
                        seq.serialize_element(item)?;
                    }
                    seq.end()
                }
                Value::Object(map) => {
                    let mut m = ser.serialize_map(Some(map.len()))?;
                    for (k, v) in map {
                        m.serialize_entry(k, v)?;
                    }
                    m.end()
                }
            }
        }
    }

    /// Decode a wire string into the appropriate `Value`. If the string
    /// carries the reserved `@bytes:` prefix, base64-decode the remainder
    /// into `Value::Bytes`; otherwise produce `Value::String`.
    fn value_from_str<E: de::Error>(s: &str) -> Result<Value, E> {
        if let Some(rest) = s.strip_prefix(BYTES_PREFIX) {
            let bytes = BASE64.decode(rest).map_err(|err| {
                de::Error::custom(format!("invalid base64 in '@bytes:' value: {err}"))
            })?;
            Ok(Value::Bytes(bytes))
        } else {
            Ok(Value::String(s.to_owned()))
        }
    }

    struct ValueVisitor;

    impl<'de> Visitor<'de> for ValueVisitor {
        type Value = Value;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a JSON-shaped value (null, bool, number, string, array, or object)")
        }

        fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
            Ok(Value::Null)
        }

        fn visit_none<E: de::Error>(self) -> Result<Value, E> {
            Ok(Value::Null)
        }

        fn visit_some<D: Deserializer<'de>>(self, de: D) -> Result<Value, D::Error> {
            de.deserialize_any(ValueVisitor)
        }

        fn visit_bool<E: de::Error>(self, v: bool) -> Result<Value, E> {
            Ok(Value::Bool(v))
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Value, E> {
            Ok(Value::Integer(v))
        }

        fn visit_i128<E: de::Error>(self, v: i128) -> Result<Value, E> {
            i64::try_from(v)
                .map(Value::Integer)
                .map_err(|_| de::Error::custom("integer out of range for i64"))
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Value, E> {
            i64::try_from(v)
                .map(Value::Integer)
                .map_err(|_| de::Error::custom("integer out of range for i64"))
        }

        fn visit_u128<E: de::Error>(self, v: u128) -> Result<Value, E> {
            i64::try_from(v)
                .map(Value::Integer)
                .map_err(|_| de::Error::custom("integer out of range for i64"))
        }

        fn visit_f64<E: de::Error>(self, v: f64) -> Result<Value, E> {
            Ok(Value::Float(v))
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Value, E> {
            value_from_str(v)
        }

        fn visit_string<E: de::Error>(self, v: String) -> Result<Value, E> {
            // Avoid the extra allocation when the string is plain.
            if v.starts_with(BYTES_PREFIX) {
                value_from_str(&v)
            } else {
                Ok(Value::String(v))
            }
        }

        fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Value, E> {
            Ok(Value::Bytes(v.to_vec()))
        }

        fn visit_byte_buf<E: de::Error>(self, v: Vec<u8>) -> Result<Value, E> {
            Ok(Value::Bytes(v))
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
            let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0));
            while let Some(item) = seq.next_element::<Value>()? {
                out.push(item);
            }
            Ok(Value::Array(out))
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
            let mut out = BTreeMap::new();
            while let Some((k, v)) = map.next_entry::<String, Value>()? {
                out.insert(k, v);
            }
            Ok(Value::Object(out))
        }
    }

    impl<'de> Deserialize<'de> for Value {
        fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
            de.deserialize_any(ValueVisitor)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn accessors_match_variants() {
        assert!(Value::Null.is_null());
        assert_eq!(Value::Bool(true).as_bool(), Some(true));
        assert_eq!(Value::Integer(42).as_integer(), Some(42));
        assert_eq!(Value::Float(1.5).as_float(), Some(1.5));
        assert_eq!(Value::String("x".into()).as_string(), Some("x"));
        assert_eq!(Value::Bytes(vec![1, 2]).as_bytes(), Some(&[1u8, 2][..]));
        assert_eq!(Value::Array(vec![Value::Null]).as_array().unwrap().len(), 1);
        let mut o = BTreeMap::new();
        o.insert("k".into(), Value::Bool(false));
        assert!(Value::Object(o).as_object().is_some());
    }

    #[test]
    fn accessors_return_none_for_other_variants() {
        assert_eq!(Value::Null.as_integer(), None);
        assert_eq!(Value::Bool(true).as_string(), None);
        assert!(!Value::Bool(false).is_null());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn bytes_round_trips_through_json() {
        let original = Value::Bytes(b"hello".to_vec());
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(json, r#""@bytes:aGVsbG8=""#);
        let back: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(original, back);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn empty_bytes_round_trips() {
        let original = Value::Bytes(Vec::new());
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(json, r#""@bytes:""#);
        let back: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(original, back);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn empty_array_round_trips_as_array_not_bytes() {
        let original = Value::Array(Vec::new());
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(json, "[]");
        let back: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(original, back);
        assert!(matches!(back, Value::Array(_)));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn array_of_small_ints_round_trips_as_array() {
        let original = Value::Array(vec![Value::Integer(1), Value::Integer(255)]);
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(json, "[1,255]");
        let back: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(original, back);
        match back {
            Value::Array(ref items) => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0], Value::Integer(1));
                assert_eq!(items[1], Value::Integer(255));
            }
            other => panic!("expected Array, got {other:?}"),
        }
    }

    #[cfg(feature = "serde")]
    #[test]
    fn string_with_reserved_prefix_rejected_on_serialize() {
        let bad = Value::String("@bytes:anything".into());
        let err = serde_json::to_string(&bad).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("@bytes:"),
            "expected error to mention reserved prefix, got: {msg}"
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn bytes_invalid_base64_rejected_on_deserialize() {
        let bad_wire = r#""@bytes:NOT-VALID-BASE64!@#""#;
        let result: Result<Value, _> = serde_json::from_str(bad_wire);
        let err = result.expect_err("invalid base64 must produce a typed deserialize error");
        let msg = err.to_string();
        assert!(
            msg.contains("base64"),
            "expected error message to mention base64, got: {msg}"
        );
    }
}
