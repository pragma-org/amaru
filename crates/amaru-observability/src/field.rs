// Copyright 2026 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Structured field encoding across the `tracing` boundary.
//!
//! ## Contract
//!
//! - **Primitives** (`bool`, integers, `f64`) and fields declared exactly as `String` are
//!   recorded with the matching typed `tracing::Value` path (`record_bool`, `record_u64`,
//!   `record_str`, …). Other string-like types (`&str`, `Cow<'_, str>`) take the CBOR path
//!   unless the schema macro transport is broadened later.
//! - **All other values** are serialized with [`cbor4ii`] via [`serde::Serialize`] and recorded
//!   with [`Visit::record_bytes`](tracing::field::Visit::record_bytes). Within Amaru every
//!   `record_bytes` payload is therefore CBOR (a bare `[u8]` is encoded as a CBOR byte string).
//!
//! Downstream layers owned by this crate decode those bytes for human/JSON/OTEL presentation.

use cbor_data::Cbor;
use opentelemetry::{Array, Key, StringValue, Value as TraceValue, logs::AnyValue};
use serde::Serialize;
use serde_json::Value as JsonValue;
use thiserror::Error;

/// Maximum length of CBOR diagnostic / JSON fallback text printed to the console.
pub const DIAG_TRUNCATION_LIMIT: usize = 512;

/// Serialize `value` as CBOR for transport through `tracing` as `record_bytes`.
///
/// Returns an owned `Box<[u8]>` so the result implements [`tracing::Value`] via
/// `Box<[u8]>` → `[u8]` → `record_bytes`.
pub fn encode_cbor<T: Serialize>(value: &T) -> Box<[u8]> {
    let mut buf = Vec::new();
    // Encoding failures are treated as empty CBOR null so instrumentation never panics.
    if cbor4ii::serde::to_writer(&mut buf, value).is_err() {
        // CBOR null
        buf.clear();
        buf.push(0xf6);
    }
    buf.into_boxed_slice()
}

/// Borrow `value` as `str` for typed string field emission (`record_str`).
#[inline]
pub fn as_str_value<T: AsRef<str> + ?Sized>(value: &T) -> &str {
    value.as_ref()
}

/// Format CBOR bytes using cbor-data's diagnostic notation (RFC 8610 style).
///
/// On parse failure, falls back to a hex dump. Output longer than
/// [`DIAG_TRUNCATION_LIMIT`] is truncated with an ellipsis.
pub fn cbor_diagnostic(bytes: &[u8]) -> String {
    let text = match Cbor::checked(bytes) {
        Ok(cbor) => cbor.to_string(),
        Err(_) => format!("h'{}'", hex::encode(bytes)),
    };
    truncate_diag(&text)
}

/// A scalar-or-text decoding of CBOR suitable for sinks that only store flat field values
/// (e.g. the TUI `FieldValue` model).
#[derive(Debug, Clone, PartialEq)]
pub enum DecodedField {
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    /// Text, diagnostic notation, or JSON text for arrays/objects.
    Text(String),
}

/// Decode CBOR into a flat [`DecodedField`] for TUI / similar consumers.
///
/// Scalars become typed values; arrays/maps become diagnostic (or JSON) text so existing
/// string-based display keeps working.
pub fn cbor_to_decoded_field(bytes: &[u8]) -> DecodedField {
    match cbor_to_json(bytes) {
        Ok(JsonValue::Bool(b)) => DecodedField::Bool(b),
        Ok(JsonValue::Number(n)) => {
            if let Some(u) = n.as_u64() {
                DecodedField::U64(u)
            } else if let Some(i) = n.as_i64() {
                DecodedField::I64(i)
            } else if let Some(f) = n.as_f64() {
                DecodedField::F64(f)
            } else {
                DecodedField::Text(n.to_string())
            }
        }
        Ok(JsonValue::String(s)) => DecodedField::Text(s),
        Ok(JsonValue::Null) => DecodedField::Text(String::new()),
        // Arrays/objects: prefer CBOR diagnostic (design for human sinks).
        Ok(JsonValue::Array(_) | JsonValue::Object(_)) => DecodedField::Text(cbor_diagnostic(bytes)),
        Err(_) => DecodedField::Text(cbor_diagnostic(bytes)),
    }
}

fn truncate_diag(text: &str) -> String {
    if text.len() <= DIAG_TRUNCATION_LIMIT {
        return text.to_owned();
    }
    let mut end = DIAG_TRUNCATION_LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Decode CBOR bytes into a [`serde_json::Value`] for structured JSON log fields.
///
/// Maps, arrays, and scalars become first-class JSON. CBOR byte strings become a JSON
/// string of lowercase hex (prefixed with `h'…'` is avoided so consumers can decode).
pub fn cbor_to_json(bytes: &[u8]) -> Result<JsonValue, CborJsonError> {
    let cbor = Cbor::checked(bytes)?;
    cbor_item_to_json(cbor)
}

/// Decode CBOR bytes into an OpenTelemetry log [`AnyValue`] (JSON-shaped nesting).
///
/// - Maps → [`AnyValue::Map`]
/// - Arrays → [`AnyValue::ListAny`]
/// - Byte strings → [`AnyValue::Bytes`]
/// - Null → empty string (OTEL has no null)
///
/// On parse failure, falls back to [`AnyValue::Bytes`] of the raw payload so data is not dropped.
pub fn cbor_to_any_value(bytes: &[u8]) -> AnyValue {
    match Cbor::checked(bytes) {
        Ok(cbor) => cbor_item_to_any(cbor).unwrap_or_else(|_| AnyValue::from(bytes)),
        Err(_) => AnyValue::from(bytes),
    }
}

/// Decode CBOR into a **trace** attribute [`TraceValue`].
///
/// Prefer a native span value when possible: a scalar or a **homogeneous** array of
/// bool / i64 / f64 / string. Maps, mixed arrays, byte strings, and invalid CBOR become a
/// [diagnostic-notation](cbor_diagnostic) string so operators never see only a hex dump
/// (stock `tracing-opentelemetry` has no `record_bytes` handler).
///
/// Nested maps for **logs** use [`cbor_to_any_value`]; classic span attributes cannot hold them.
pub fn cbor_to_trace_value(bytes: &[u8]) -> TraceValue {
    match Cbor::checked(bytes).ok().and_then(cbor_item_to_trace_value) {
        Some(value) => value,
        None => TraceValue::String(cbor_diagnostic(bytes).into()),
    }
}

fn cbor_item_to_trace_value(cbor: &Cbor) -> Option<TraceValue> {
    if cbor.try_null().is_ok() {
        return None;
    }
    if let Ok(b) = cbor.try_bool() {
        return Some(TraceValue::Bool(b));
    }
    if let Ok(n) = cbor.try_number() {
        return number_to_trace_value(n);
    }
    if let Ok(s) = cbor.try_str() {
        return Some(TraceValue::String(s.into_owned().into()));
    }
    if cbor.try_bytes().is_ok() {
        // Trace Value has no Bytes variant.
        return None;
    }
    if let Ok(items) = cbor.try_array() {
        return homogeneous_array_to_trace_value(&items);
    }
    // Maps and other kinds are not representable as classic TraceValue.
    None
}

fn number_to_trace_value(n: cbor_data::value::Number<'_>) -> Option<TraceValue> {
    use cbor_data::value::Number;
    match n {
        Number::Int(i) => i64::try_from(i).ok().map(TraceValue::I64),
        Number::IEEE754(f) => Some(TraceValue::F64(f)),
        Number::Decimal(_) | Number::Float(_) => None,
    }
}

/// Classify a CBOR array as a homogeneous `Value::Array`, or `None` if mixed / nested.
fn homogeneous_array_to_trace_value(items: &[std::borrow::Cow<'_, Cbor>]) -> Option<TraceValue> {
    use cbor_data::value::Number;

    if items.is_empty() {
        // Empty array: prefer empty string array (common for tag lists).
        return Some(TraceValue::Array(Array::String(Vec::new())));
    }

    // Probe first element kind, then require all match.
    let first = items[0].as_ref();
    if first.try_bool().is_ok() {
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            out.push(item.as_ref().try_bool().ok()?);
        }
        return Some(TraceValue::Array(Array::Bool(out)));
    }
    if first.try_str().is_ok() {
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            let s = item.as_ref().try_str().ok()?;
            out.push(StringValue::from(s.into_owned()));
        }
        return Some(TraceValue::Array(Array::String(out)));
    }
    match first.try_number() {
        Ok(Number::IEEE754(_)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item.as_ref().try_number().ok()? {
                    Number::IEEE754(f) => out.push(f),
                    Number::Int(_) | Number::Decimal(_) | Number::Float(_) => return None,
                }
            }
            Some(TraceValue::Array(Array::F64(out)))
        }
        Ok(Number::Int(_)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item.as_ref().try_number().ok()? {
                    Number::Int(i) => out.push(i64::try_from(i).ok()?),
                    Number::IEEE754(_) | Number::Decimal(_) | Number::Float(_) => return None,
                }
            }
            Some(TraceValue::Array(Array::I64(out)))
        }
        Ok(Number::Decimal(_) | Number::Float(_)) | Err(_) => None,
    }
}

fn cbor_item_to_any(cbor: &Cbor) -> Result<AnyValue, CborJsonError> {
    if cbor.try_null().is_ok() {
        // OTEL has no null; use empty string for presence without a value.
        return Ok(AnyValue::from(""));
    }
    if let Ok(b) = cbor.try_bool() {
        return Ok(AnyValue::from(b));
    }
    if let Ok(n) = cbor.try_number() {
        return Ok(number_to_any(n));
    }
    if let Ok(s) = cbor.try_str() {
        return Ok(AnyValue::from(s.into_owned()));
    }
    if let Ok(raw) = cbor.try_bytes() {
        return Ok(AnyValue::from(raw.as_ref()));
    }
    if let Ok(items) = cbor.try_array() {
        let mut arr = Vec::with_capacity(items.len());
        for item in items {
            arr.push(cbor_item_to_any(item.as_ref())?);
        }
        return Ok(AnyValue::ListAny(Box::new(arr)));
    }
    if let Ok(dict) = cbor.try_dict() {
        // Build via FromIterator so we don't construct std::collections::HashMap here
        // (disallowed by project clippy rules); opentelemetry's FromIterator uses HashMap internally.
        let pairs: Vec<(Key, AnyValue)> = dict
            .into_iter()
            .map(|(key, value)| {
                let key_str = dict_key_to_string(key.as_ref())?;
                let val = cbor_item_to_any(value.as_ref())?;
                Ok((Key::new(key_str), val))
            })
            .collect::<Result<Vec<_>, CborJsonError>>()?;
        return Ok(AnyValue::from_iter(pairs));
    }
    if let Ok(ts) = cbor.try_timestamp() {
        return Ok(AnyValue::from(format!("{ts:?}")));
    }
    Ok(AnyValue::from(cbor_diagnostic(cbor.as_slice())))
}

fn number_to_any(n: cbor_data::value::Number<'_>) -> AnyValue {
    use cbor_data::value::Number;
    match n {
        Number::Int(i) => {
            if let Ok(s) = i64::try_from(i) {
                AnyValue::from(s)
            } else {
                AnyValue::from(i.to_string())
            }
        }
        Number::IEEE754(f) => AnyValue::from(f),
        Number::Decimal(d) => AnyValue::from(format!("{d:?}")),
        Number::Float(f) => AnyValue::from(format!("{f:?}")),
    }
}

#[derive(Debug, Error)]
pub enum CborJsonError {
    #[error("invalid CBOR: {0}")]
    Parse(#[from] cbor_data::ParseError),
    #[error("unsupported CBOR for JSON: {0}")]
    Type(String),
}

fn cbor_item_to_json(cbor: &Cbor) -> Result<JsonValue, CborJsonError> {
    // Prefer high-level try_* accessors when they succeed.
    if cbor.try_null().is_ok() {
        return Ok(JsonValue::Null);
    }
    if let Ok(b) = cbor.try_bool() {
        return Ok(JsonValue::Bool(b));
    }
    if let Ok(n) = cbor.try_number() {
        return Ok(number_to_json(n));
    }
    if let Ok(s) = cbor.try_str() {
        return Ok(JsonValue::String(s.into_owned()));
    }
    if let Ok(bytes) = cbor.try_bytes() {
        return Ok(JsonValue::String(hex::encode(bytes.as_ref())));
    }
    if let Ok(items) = cbor.try_array() {
        let mut arr = Vec::with_capacity(items.len());
        for item in items {
            arr.push(cbor_item_to_json(item.as_ref())?);
        }
        return Ok(JsonValue::Array(arr));
    }
    if let Ok(dict) = cbor.try_dict() {
        let mut map = serde_json::Map::new();
        for (key, value) in dict {
            let key_str = dict_key_to_string(key.as_ref())?;
            map.insert(key_str, cbor_item_to_json(value.as_ref())?);
        }
        return Ok(JsonValue::Object(map));
    }
    if let Ok(ts) = cbor.try_timestamp() {
        return Ok(timestamp_to_json(ts));
    }

    // Fallback: diagnostic notation for tags / simple values we don't map.
    Ok(JsonValue::String(cbor_diagnostic(cbor.as_slice())))
}

fn number_to_json(n: cbor_data::value::Number<'_>) -> JsonValue {
    use cbor_data::value::Number;
    match n {
        Number::Int(i) => {
            if let Ok(u) = u64::try_from(i) {
                JsonValue::Number(u.into())
            } else if let Ok(s) = i64::try_from(i) {
                JsonValue::Number(s.into())
            } else {
                JsonValue::String(i.to_string())
            }
        }
        Number::IEEE754(f) => serde_json::Number::from_f64(f).map(JsonValue::Number).unwrap_or(JsonValue::Null),
        Number::Decimal(d) => JsonValue::String(format!("{d:?}")),
        Number::Float(f) => JsonValue::String(format!("{f:?}")),
    }
}

fn timestamp_to_json(ts: cbor_data::value::Timestamp) -> JsonValue {
    JsonValue::String(format!("{ts:?}"))
}

fn dict_key_to_string(key: &Cbor) -> Result<String, CborJsonError> {
    if let Ok(s) = key.try_str() {
        return Ok(s.into_owned());
    }
    if let Ok(n) = key.try_number() {
        return Ok(match n {
            cbor_data::value::Number::Int(i) => i.to_string(),
            cbor_data::value::Number::IEEE754(f) => f.to_string(),
            cbor_data::value::Number::Decimal(d) => format!("{d:?}"),
            cbor_data::value::Number::Float(f) => format!("{f:?}"),
        });
    }
    Ok(cbor_diagnostic(key.as_slice()))
}

#[cfg(test)]
mod tests {
    use serde::Serialize;

    use super::*;

    #[derive(Serialize)]
    struct Sample {
        peers: Vec<String>,
        count: u64,
        ok: bool,
    }

    #[test]
    fn encode_cbor_roundtrip_json_array_and_object() {
        let sample = Sample { peers: vec!["a:1".into(), "b:2".into()], count: 7, ok: true };
        let bytes = encode_cbor(&sample);
        let json = cbor_to_json(&bytes).expect("decode");
        assert_eq!(json["count"], 7);
        assert_eq!(json["ok"], true);
        assert_eq!(json["peers"], serde_json::json!(["a:1", "b:2"]));
    }

    #[test]
    fn primitives_encode_and_decode() {
        assert_eq!(cbor_to_json(&encode_cbor(&true)).unwrap(), JsonValue::Bool(true));
        assert_eq!(cbor_to_json(&encode_cbor(&42u64)).unwrap(), JsonValue::Number(42.into()));
        assert_eq!(cbor_to_json(&encode_cbor(&"hello")).unwrap(), JsonValue::String("hello".into()));
    }

    #[test]
    fn diagnostic_is_non_empty() {
        let bytes = encode_cbor(&vec![1u32, 2, 3]);
        let diag = cbor_diagnostic(&bytes);
        assert!(!diag.is_empty());
        assert!(diag.contains('1') || diag.contains('['));
    }

    #[test]
    fn cbor_to_decoded_field_preserves_scalars() {
        assert_eq!(cbor_to_decoded_field(&encode_cbor(&true)), DecodedField::Bool(true));
        assert_eq!(cbor_to_decoded_field(&encode_cbor(&42u64)), DecodedField::U64(42));
        assert_eq!(cbor_to_decoded_field(&encode_cbor(&"hello")), DecodedField::Text("hello".into()));
    }

    #[test]
    fn cbor_to_decoded_field_arrays_use_diagnostic_not_raw_bytes_debug() {
        let bytes = encode_cbor(&vec![1u32, 2, 3]);
        let DecodedField::Text(text) = cbor_to_decoded_field(&bytes) else {
            panic!("expected text for array");
        };
        // Stock Visit::record_bytes falls through to Debug of the raw byte slice.
        assert_ne!(text, format!("{:?}", bytes.as_ref()), "must not be raw bytes Debug");
        assert!(text.contains('1') && text.contains('2'), "diagnostic={text}");
    }

    #[test]
    fn cbor_to_trace_value_preserves_homogeneous_arrays() {
        use opentelemetry::Array;

        let strings = cbor_to_trace_value(&encode_cbor(&vec!["a:1".to_string(), "b:2".to_string()]));
        let TraceValue::Array(Array::String(items)) = strings else {
            panic!("expected string array, got {strings:?}");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].as_str(), "a:1");

        let ints = cbor_to_trace_value(&encode_cbor(&vec![1_i64, 2, 3]));
        let TraceValue::Array(Array::I64(items)) = ints else {
            panic!("expected i64 array, got {ints:?}");
        };
        assert_eq!(items, vec![1, 2, 3]);

        let empty = cbor_to_trace_value(&encode_cbor(&Vec::<String>::new()));
        let TraceValue::Array(Array::String(items)) = empty else {
            panic!("expected empty string array, got {empty:?}");
        };
        assert!(items.is_empty());
    }

    #[test]
    fn cbor_to_trace_value_maps_and_mixed_use_diagnostic() {
        #[derive(Serialize)]
        struct M {
            a: u64,
        }
        let map = cbor_to_trace_value(&encode_cbor(&M { a: 1 }));
        let TraceValue::String(s) = map else {
            panic!("expected diagnostic for map, got {map:?}");
        };
        assert!(!s.as_str().is_empty());

        let mixed = cbor_to_trace_value(&encode_cbor(&(1_u64, "x")));
        let TraceValue::String(s) = mixed else {
            panic!("expected diagnostic for mixed array, got {mixed:?}");
        };
        assert!(!s.as_str().is_empty());
    }

    #[test]
    fn cbor_to_any_value_builds_nested_map_and_list() {
        use opentelemetry::{Key, logs::AnyValue};

        let sample = Sample { peers: vec!["a:1".into(), "b:2".into()], count: 7, ok: true };
        let any = cbor_to_any_value(&encode_cbor(&sample));
        match any {
            AnyValue::Map(map) => {
                assert!(matches!(map.get(&Key::new("count")), Some(AnyValue::Int(7))));
                assert!(matches!(map.get(&Key::new("ok")), Some(AnyValue::Boolean(true))));
                assert!(matches!(map.get(&Key::new("peers")), Some(AnyValue::ListAny(list)) if list.len() == 2));
            }
            AnyValue::Int(_)
            | AnyValue::Double(_)
            | AnyValue::String(_)
            | AnyValue::Boolean(_)
            | AnyValue::Bytes(_)
            | AnyValue::ListAny(_) => panic!("expected Map, got {any:?}"),
            // non_exhaustive
            _ => panic!("expected Map, got unexpected AnyValue variant"),
        }
    }
}
