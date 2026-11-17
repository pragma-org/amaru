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

use std::{fs, io, path::Path};

use amaru_kernel::cbor;
use amaru_observability::trace;
use serde::Deserialize;
use thiserror::Error;
use zeroize::Zeroizing;

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
    let path = file.as_ref();
    let bytes = Zeroizing::new(fs::read(path)?);
    decode_envelope(path, &bytes, ctx)
}

fn decode_envelope<C, T: for<'d> cbor::Decode<'d, C> + HasEnvelope>(
    path: &Path,
    bytes: &[u8],
    ctx: &mut C,
) -> Result<T, ReadEnvelopeError> {
    let envelope: Envelope = serde_json::from_slice(bytes)?;
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
    let value = decode_cbor(&payload, ctx)?;
    trace!(
        io::envelope::LEGACY,
        envelope_type = envelope.r#type.as_deref().unwrap_or_default(),
        path = path.to_string_lossy()
    );
    Ok(value)
}

fn decode_cbor<C, T: for<'d> cbor::Decode<'d, C>>(bytes: &[u8], ctx: &mut C) -> Result<T, ReadEnvelopeError> {
    let mut decoder = cbor::Decoder::new(bytes);
    let value = decoder.decode_with(ctx)?;
    if decoder.position() != bytes.len() {
        return Err(ReadEnvelopeError::TrailingBytes(bytes.len() - decoder.position()));
    }
    Ok(value)
}

/// Read binary CBOR or a legacy cardano-cli JSON envelope.
/// Both formats must decode completely, without trailing bytes.
pub fn read_cbor_or_envelope<C, T: for<'d> cbor::Decode<'d, C> + HasEnvelope>(
    path: impl AsRef<Path>,
    ctx: &mut C,
) -> Result<T, ReadEnvelopeError> {
    let path = path.as_ref();
    let bytes = Zeroizing::new(fs::read(path)?);
    if bytes.iter().find(|byte| !byte.is_ascii_whitespace()) == Some(&b'{') {
        return decode_envelope(path, &bytes, ctx);
    }
    decode_cbor(&bytes, ctx)
}

/// Read an exact-size raw key or a legacy cardano-cli envelope.
/// File contents are zeroed on drop. Successfully decoded envelopes emit a trace with their type and path.
pub fn read_key_file<const SIZE: usize, C, T, E>(
    path: &Path,
    ctx: &mut C,
    from_raw: impl FnOnce(&[u8; SIZE]) -> Result<T, E>,
) -> Result<T, E>
where
    T: for<'d> cbor::Decode<'d, C> + HasEnvelope,
    E: From<io::Error> + From<ReadEnvelopeError>,
{
    let bytes = Zeroizing::new(fs::read(path)?);
    if let Ok(raw) = <&[u8; SIZE]>::try_from(bytes.as_slice()) {
        return from_raw(raw);
    }
    Ok(decode_envelope(path, &bytes, ctx)?)
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
