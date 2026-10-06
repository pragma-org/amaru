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

//! The JSON text envelope cardano-cli wraps keys, certificates, and other CBOR values in:
//!
//! ```json
//! {
//!     "type": "KesVerificationKey_ed25519_kes_2^6",
//!     "description": "KES Verification Key",
//!     "cborHex": "5820..."
//! }
//! ```
//!
//! Every intermediate buffer holding the file, the hex, or the CBOR payload has the type's
//! `Buffer` type, so secret keys can be wiped on drop while public values are not.

use std::{borrow::Cow, convert::Infallible, fs, io, path::Path};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::cbor;

/// A value read from a text envelope.
pub trait FromTextEnvelope: Sized {
    /// `type` values accepted on read.
    const TYPES: &'static [&'static str];

    /// Storage for the file bytes and the decoded payload
    type Buffer: From<Vec<u8>> + AsMut<Vec<u8>>;

    type Error: From<TextEnvelopeError>;

    /// Decode the payload of an envelope whose `type` is `r#type`, one of [`Self::TYPES`].
    fn decode_cbor(r#type: &'static str, decoder: &mut cbor::Decoder<'_>) -> Result<Self, Self::Error>;
}

/// A value written as a text envelope.
pub trait ToTextEnvelope {
    /// Storage for the encoded payload and its hex: `Zeroizing<Vec<u8>>` for secrets, `Vec<u8>` otherwise.
    type Buffer: From<Vec<u8>> + AsMut<Vec<u8>>;

    fn type_name(&self) -> &'static str;

    fn description(&self) -> &'static str {
        ""
    }
    fn encode_cbor<W: cbor::encode::Write>(
        &self,
        encoder: &mut cbor::Encoder<W>,
    ) -> Result<(), cbor::encode::Error<W::Error>>;
}

/// Read and decode a text envelope file.
pub fn read<T: FromTextEnvelope>(path: impl AsRef<Path>) -> Result<T, T::Error> {
    let mut json = T::Buffer::from(fs::read(path).map_err(TextEnvelopeError::Io)?);
    from_json(json.as_mut())
}

/// Decode a text envelope from its JSON bytes.
pub fn from_json<T: FromTextEnvelope>(json: &[u8]) -> Result<T, T::Error> {
    let Envelope { r#type, cbor_hex, .. } = serde_json::from_slice(json).map_err(TextEnvelopeError::Json)?;
    let Some(r#type) = T::TYPES.iter().copied().find(|expected| *expected == r#type) else {
        return Err(TextEnvelopeError::UnexpectedType { expected: T::TYPES, found: r#type.into_owned() }.into());
    };
    let mut payload = T::Buffer::from(vec![0u8; cbor_hex.len() / 2]);
    let payload = payload.as_mut();
    hex::decode_to_slice(cbor_hex.as_ref(), payload).map_err(TextEnvelopeError::Hex)?;
    let mut decoder = cbor::Decoder::new(payload);
    let value = T::decode_cbor(r#type, &mut decoder)?;
    if decoder.position() != payload.len() {
        return Err(TextEnvelopeError::TrailingBytes(payload.len() - decoder.position()).into());
    }
    Ok(value)
}

/// Write `value` as a text envelope, formatted as cardano-cli formats it.
pub fn write<T: ToTextEnvelope>(value: &T, writer: impl io::Write) -> Result<(), TextEnvelopeError> {
    let mut size = ByteCount(0);
    value.encode_cbor(&mut cbor::Encoder::new(&mut size))?;
    let mut payload = T::Buffer::from(Vec::with_capacity(size.0));
    let payload = payload.as_mut();
    value.encode_cbor(&mut cbor::Encoder::new(&mut *payload))?;

    let mut cbor_hex = T::Buffer::from(vec![0u8; payload.len() * 2]);
    let cbor_hex = cbor_hex.as_mut();
    hex::encode_to_slice(&*payload, cbor_hex)
        .unwrap_or_else(|e| unreachable!("Impossible! hex buffer is sized to twice the payload: {e}"));
    let cbor_hex =
        std::str::from_utf8(cbor_hex).unwrap_or_else(|e| unreachable!("Impossible! hex encoding is always ASCII: {e}"));

    let envelope = Envelope {
        r#type: Cow::Borrowed(value.type_name()),
        description: Cow::Borrowed(value.description()),
        cbor_hex: Cow::Borrowed(cbor_hex),
    };
    let mut serializer =
        serde_json::Serializer::with_formatter(writer, serde_json::ser::PrettyFormatter::with_indent(b"    "));
    envelope.serialize(&mut serializer)?;
    io::Write::write_all(&mut serializer.into_inner(), b"\n")?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope<'a> {
    #[serde(borrow)]
    r#type: Cow<'a, str>,
    #[serde(borrow, default)]
    description: Cow<'a, str>,
    #[serde(borrow)]
    cbor_hex: Cow<'a, str>,
}

struct ByteCount(usize);

impl cbor::encode::Write for ByteCount {
    type Error = Infallible;

    fn write_all(&mut self, buf: &[u8]) -> Result<(), Self::Error> {
        self.0 += buf.len();
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum TextEnvelopeError {
    #[error("failed to access text envelope file: {0}")]
    Io(#[from] io::Error),
    #[error("malformed text envelope: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unexpected text envelope type: expected {}, found {found}", expected.join(" or "))]
    UnexpectedType { expected: &'static [&'static str], found: String },
    #[error("malformed text envelope cborHex: {0}")]
    Hex(#[from] hex::FromHexError),
    #[error("malformed text envelope payload: {0}")]
    Decode(#[from] cbor::decode::Error),
    #[error("failed to encode text envelope payload: {0}")]
    Encode(#[from] cbor::encode::Error<Infallible>),
    #[error("text envelope payload has {0} trailing bytes")]
    TrailingBytes(usize),
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    #[derive(Debug, PartialEq)]
    struct Bytes(Vec<u8>);

    impl FromTextEnvelope for Bytes {
        const TYPES: &'static [&'static str] = &["Bytes", "LegacyBytes"];
        type Buffer = Vec<u8>;
        type Error = TextEnvelopeError;

        fn decode_cbor(_type: &'static str, decoder: &mut cbor::Decoder<'_>) -> Result<Self, Self::Error> {
            Ok(Self(cbor::decode_bytes(decoder)?.into_owned()))
        }
    }

    impl ToTextEnvelope for Bytes {
        type Buffer = Vec<u8>;

        fn type_name(&self) -> &'static str {
            "Bytes"
        }

        fn description(&self) -> &'static str {
            "Some bytes"
        }

        fn encode_cbor<W: cbor::encode::Write>(
            &self,
            encoder: &mut cbor::Encoder<W>,
        ) -> Result<(), cbor::encode::Error<W::Error>> {
            encoder.bytes(&self.0)?;
            Ok(())
        }
    }

    fn envelope(r#type: &str, cbor_hex: &str) -> String {
        format!(r#"{{"type":"{type}","description":"","cborHex":"{cbor_hex}"}}"#)
    }

    #[test]
    fn writes_the_cardano_cli_layout() {
        let mut out = Vec::new();
        write(&Bytes(vec![0xab, 0xcd]), &mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "{\n    \"type\": \"Bytes\",\n    \"description\": \"Some bytes\",\n    \"cborHex\": \"42abcd\"\n}\n"
        );
    }

    #[test_case(&envelope("Other", "40"), "expected Bytes or LegacyBytes, found Other"; "wrong type")]
    #[test_case(&envelope("Bytes", "4"), "Odd number of digits"; "odd hex length")]
    #[test_case(&envelope("Bytes", "4000"), "1 trailing bytes"; "trailing bytes")]
    #[test_case(&envelope("Bytes", "00"), "unexpected type"; "payload is not a byte string")]
    #[test_case(r#"{"cborHex":"40"}"#, "missing field `type`"; "missing type")]
    #[test_case(r#"{"type":"Bytes"}"#, "missing field `cborHex`"; "missing cborHex")]
    fn from_json_rejects(json: &str, expected_error: &str) {
        let err = from_json::<Bytes>(json.as_bytes()).unwrap_err().to_string();
        assert!(err.contains(expected_error), "{err}");
    }

    #[test]
    fn read_reports_missing_file() {
        let err = read::<Bytes>("/nonexistent/bytes.json").unwrap_err();
        assert!(matches!(err, TextEnvelopeError::Io(_)), "{err}");
    }
}
