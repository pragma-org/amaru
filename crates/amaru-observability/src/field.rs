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
//! - **Primitives** (`bool`, integers, `f64`, `String` / `AsRef<str>`) are recorded with the
//!   matching typed `tracing::Value` path (`record_bool`, `record_u64`, `record_str`, …).
//! - **All other values** are serialized with [`cbor4ii`] via [`serde::Serialize`] and recorded
//!   with [`Visit::record_bytes`](tracing::field::Visit::record_bytes). Within Amaru every
//!   `record_bytes` payload is therefore CBOR (a bare `[u8]` is encoded as a CBOR byte string).
//!
//! Downstream layers owned by this crate decode those bytes for human/JSON/OTEL presentation.

use std::fmt;

use cbor_data::Cbor;
use opentelemetry::{Key, logs::AnyValue};
use serde::Serialize;
use serde_json::Value as JsonValue;

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
    let cbor = Cbor::checked(bytes).map_err(CborJsonError::Parse)?;
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
            } else if let Ok(u) = u64::try_from(i) {
                // Prefer i64 for OTEL; large u64 as string.
                if let Ok(s) = i64::try_from(u) { AnyValue::from(s) } else { AnyValue::from(u.to_string()) }
            } else {
                AnyValue::from(i.to_string())
            }
        }
        Number::IEEE754(f) => AnyValue::from(f),
        Number::Decimal(d) => AnyValue::from(format!("{d:?}")),
        Number::Float(f) => AnyValue::from(format!("{f:?}")),
    }
}

#[derive(Debug)]
pub enum CborJsonError {
    Parse(cbor_data::ParseError),
    Type(String),
}

impl fmt::Display for CborJsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "invalid CBOR: {e}"),
            Self::Type(msg) => write!(f, "unsupported CBOR for JSON: {msg}"),
        }
    }
}

impl std::error::Error for CborJsonError {}

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
