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
//! The CBOR and hex buffers use the value's `Buffer` type, so secret keys can be wiped on
//! drop while public values are not.

use std::{convert::Infallible, io};

use serde::Serialize;
use thiserror::Error;

use crate::cbor;

/// A value written as a text envelope.
pub trait ToTextEnvelope {
    /// Storage for the encoded payload and its hex: `Zeroizing<Vec<u8>>` for secrets, `Vec<u8>` otherwise.
    type Buffer: From<Vec<u8>> + AsMut<Vec<u8>>;

    /// The cardano-cli envelope type.
    const TYPE: &'static str;

    /// A human-readable description of the value.
    const DESCRIPTION: &'static str = "";

    /// Encode deterministically: writing counts bytes before allocating the payload buffer.
    fn encode_cbor<W: cbor::encode::Write>(
        &self,
        encoder: &mut cbor::Encoder<W>,
    ) -> Result<(), cbor::encode::Error<W::Error>>;
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

    let envelope = Envelope { r#type: T::TYPE, description: T::DESCRIPTION, cbor_hex };
    let mut serializer =
        serde_json::Serializer::with_formatter(writer, serde_json::ser::PrettyFormatter::with_indent(b"    "));
    envelope.serialize(&mut serializer)?;
    io::Write::write_all(&mut serializer.into_inner(), b"\n")?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Envelope<'a> {
    r#type: &'a str,
    description: &'a str,
    cbor_hex: &'a str,
}

/// Counts encoded bytes to avoid reallocating buffers that hold secrets.
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
    #[error("failed to write text envelope: {0}")]
    Io(#[from] io::Error),
    #[error("failed to serialize text envelope: {0}")]
    Json(#[from] serde_json::Error),
    #[error("failed to encode text envelope payload: {0}")]
    Encode(#[from] cbor::encode::Error<Infallible>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct Bytes(Vec<u8>);

    impl ToTextEnvelope for Bytes {
        type Buffer = Vec<u8>;

        const TYPE: &'static str = "Bytes";
        const DESCRIPTION: &'static str = "Some bytes";

        fn encode_cbor<W: cbor::encode::Write>(
            &self,
            encoder: &mut cbor::Encoder<W>,
        ) -> Result<(), cbor::encode::Error<W::Error>> {
            encoder.bytes(&self.0)?;
            Ok(())
        }
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
}
