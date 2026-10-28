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

//! Reading of cardano-cli text envelope files.
//!
//! A text envelope is a JSON file with a `type` field naming the wrapped entity and a
//! `cborHex` field holding the hex-encoded CBOR serialisation of that entity.

use std::{fs, path::Path};

use serde::Deserialize;
use thiserror::Error;
use zeroize::Zeroizing;

use crate::cbor;

/// An entity that cardano-cli writes as a text envelope.
pub trait HasEnvelope {
    /// `type` field values accepted for this entity. cardano-cli writes more than one
    /// name for some entities. An empty list accepts any `type`, including none, for
    /// legacy files whose field was never checked.
    const ACCEPTED_TYPES: &'static [&'static str];
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    #[serde(default)]
    r#type: Option<String>,
    cbor_hex: Zeroizing<String>,
}

/// Read a cardano-cli text envelope file and decode the entity it wraps.
///
/// The `type` field must be one of [`HasEnvelope::ACCEPTED_TYPES`], and the CBOR payload
/// must decode as `T` with no bytes left over. Intermediate buffers are zeroed on drop,
/// so the helper also suits signing keys.
pub fn read_envelope<C, T: for<'d> cbor::Decode<'d, C> + HasEnvelope>(
    file: impl AsRef<Path>,
    ctx: &mut C,
) -> Result<T, ReadEnvelopeError> {
    let bytes = Zeroizing::new(fs::read(file)?);
    let envelope: Envelope = serde_json::from_slice(&bytes)?;
    if !T::ACCEPTED_TYPES.is_empty() {
        match envelope.r#type {
            Some(ref found) if T::ACCEPTED_TYPES.contains(&found.as_str()) => {}
            found => {
                return Err(ReadEnvelopeError::UnexpectedType {
                    accepted: T::ACCEPTED_TYPES,
                    found: found.unwrap_or_default(),
                });
            }
        }
    }
    if !envelope.cbor_hex.len().is_multiple_of(2) {
        return Err(hex::FromHexError::OddLength.into());
    }
    let mut payload = Zeroizing::new(vec![0u8; envelope.cbor_hex.len() / 2]);
    hex::decode_to_slice(envelope.cbor_hex.as_bytes(), &mut payload)?;
    let mut decoder = cbor::Decoder::new(&payload);
    let value = decoder.decode_with(ctx)?;
    if decoder.position() != payload.len() {
        return Err(ReadEnvelopeError::TrailingBytes(payload.len() - decoder.position()));
    }
    Ok(value)
}

/// The text envelope file could not be read or its payload was refused.
#[derive(Debug, Error)]
pub enum ReadEnvelopeError {
    #[error("unexpected envelope type: expected one of {accepted:?}, found {found:?}")]
    UnexpectedType { accepted: &'static [&'static str], found: String },
    #[error("envelope payload has {0} trailing bytes")]
    TrailingBytes(usize),
    #[error("failed to read file: {0}")]
    Io(#[from] std::io::Error),
    #[error("malformed envelope: {0}")]
    Json(#[from] serde_json::Error),
    #[error("envelope hex is malformed: {0}")]
    Hex(#[from] hex::FromHexError),
    #[error("envelope CBOR is malformed: {0}")]
    Cbor(#[from] cbor::decode::Error),
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    struct Strict(u8);

    impl HasEnvelope for Strict {
        const ACCEPTED_TYPES: &'static [&'static str] = &["Primary", "Alias"];
    }

    impl<'d, C> cbor::Decode<'d, C> for Strict {
        fn decode(d: &mut cbor::Decoder<'d>, _ctx: &mut C) -> Result<Self, cbor::decode::Error> {
            Ok(Strict(d.u8()?))
        }
    }

    struct Lenient(u8);

    impl HasEnvelope for Lenient {
        const ACCEPTED_TYPES: &'static [&'static str] = &[];
    }

    impl<'d, C> cbor::Decode<'d, C> for Lenient {
        fn decode(d: &mut cbor::Decoder<'d>, _ctx: &mut C) -> Result<Self, cbor::decode::Error> {
            Ok(Lenient(d.u8()?))
        }
    }

    fn write(content: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.json");
        fs::write(&path, content).unwrap();
        (dir, path)
    }

    #[test]
    fn accepts_every_listed_type() {
        for r#type in Strict::ACCEPTED_TYPES {
            let (_dir, path) = write(&format!(r#"{{"type":"{type}","description":"","cborHex":"07"}}"#));
            let value: Strict = read_envelope(&path, &mut ()).unwrap();
            assert_eq!(value.0, 7);
        }
    }

    #[test_case(r#"{"type":"Other","cborHex":"07"}"# => matches ReadEnvelopeError::UnexpectedType { ref found, .. } if found == "Other")]
    #[test_case(r#"{"cborHex":"07"}"# => matches ReadEnvelopeError::UnexpectedType { ref found, .. } if found.is_empty())]
    fn rejects_unlisted_and_missing_types(bytes: &str) -> ReadEnvelopeError {
        let (_dir, path) = write(bytes);
        read_envelope::<_, Strict>(&path, &mut ()).map(|_| ()).unwrap_err()
    }

    #[test]
    fn empty_accepted_list_ignores_the_type() {
        let (_dir, path) = write(r#"{"type":"Whatever","cborHex":"07"}"#);
        assert_eq!(read_envelope::<_, Lenient>(&path, &mut ()).unwrap().0, 7);

        let (_dir, path) = write(r#"{"cborHex":"07"}"#);
        assert_eq!(read_envelope::<_, Lenient>(&path, &mut ()).unwrap().0, 7);
    }

    #[test]
    fn rejects_non_existing_file() {
        let err = read_envelope::<_, Lenient>("/nonexistent/file.json", &mut ()).map(|_| ()).unwrap_err();
        assert!(matches!(err, ReadEnvelopeError::Io(_)), "{err}");
    }

    #[test_case("not json" => matches ReadEnvelopeError::Json(_))]
    #[test_case(r#"{"cborHex":"0"}"# => matches ReadEnvelopeError::Hex(hex::FromHexError::OddLength))]
    #[test_case(r#"{"cborHex":"zz"}"# => matches ReadEnvelopeError::Hex(_))]
    #[test_case(r#"{"cborHex":""}"# => matches ReadEnvelopeError::Cbor(_))]
    #[test_case(r#"{"cborHex":"0700"}"# => matches ReadEnvelopeError::TrailingBytes(_))]
    fn rejects_malformed_files(bytes: &str) -> ReadEnvelopeError {
        let (_dir, path) = write(bytes);
        read_envelope::<_, Lenient>(&path, &mut ()).map(|_| ()).unwrap_err()
    }
}
