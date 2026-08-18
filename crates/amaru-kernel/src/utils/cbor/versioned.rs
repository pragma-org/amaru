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

use std::borrow::Cow;

use crate::{ProtocolParameters, ProtocolVersion, cbor, protocol_version, protocol_version::PROTOCOL_VERSION_12};

/// Expose the protocol version a CBOR decoding context operates at, so that decoders can
/// adapt their logic to the current protocol version.
pub trait HasProtocolVersion {
    fn protocol_version(&self) -> ProtocolVersion;
}

impl HasProtocolVersion for ProtocolVersion {
    fn protocol_version(&self) -> ProtocolVersion {
        *self
    }
}

/// By default, decoding without an explicit protocol version assumes the current era's version.
impl HasProtocolVersion for () {
    fn protocol_version(&self) -> ProtocolVersion {
        protocol_version::DEFAULT
    }
}

impl HasProtocolVersion for ProtocolParameters {
    fn protocol_version(&self) -> ProtocolVersion {
        self.protocol_version
    }
}

/// Decode a bytestring:
///
///  - Definite-length only below protocol version 12
///  - Indefinite-length (chunked) form from version 12 onwards.
///
/// This mirrors the `decodeBytes` function in the Haskell Cardano node.
///
/// See <https://github.com/IntersectMBO/cardano-ledger/blob/master/libs/cardano-ledger-binary/src/Cardano/Ledger/Binary/Decoding/Decoder.hs>
/// (`decodeBytes = ifDecoderVersionAtLeast (natVersion @12) ...`).
pub fn decode_bytes_with<'b, C: HasProtocolVersion>(
    d: &mut cbor::Decoder<'b>,
    ctx: &C,
) -> Result<Cow<'b, [u8]>, cbor::decode::Error> {
    if ctx.protocol_version() >= PROTOCOL_VERSION_12 {
        amaru_minicbor_extra::decode_bytes(d)
    } else {
        #[allow(clippy::disallowed_methods)]
        Ok(Cow::Borrowed(d.bytes()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{cbor, protocol_version::PROTOCOL_VERSION_11};

    // (_ h'0102', h'0304')
    const CHUNKED: &[u8] = &[0x5f, 0x42, 0x01, 0x02, 0x42, 0x03, 0x04, 0xff];
    // h'01020304'
    const DEFINITE: &[u8] = &[0x44, 0x01, 0x02, 0x03, 0x04];

    #[test]
    fn definite_bytes_decode_at_any_version() {
        for version in [PROTOCOL_VERSION_11, PROTOCOL_VERSION_12] {
            let mut d = cbor::Decoder::new(DEFINITE);
            assert_eq!(decode_bytes_with(&mut d, &version).unwrap().as_ref(), [1, 2, 3, 4]);
        }
    }

    #[test]
    fn indefinite_bytes_rejected_below_version_12() {
        let mut d = cbor::Decoder::new(CHUNKED);
        assert!(decode_bytes_with(&mut d, &PROTOCOL_VERSION_11).is_err());
    }

    #[test]
    fn indefinite_bytes_accepted_from_version_12() {
        let mut d = cbor::Decoder::new(CHUNKED);
        assert_eq!(decode_bytes_with(&mut d, &PROTOCOL_VERSION_12).unwrap().as_ref(), [1, 2, 3, 4]);
    }

    #[test]
    fn unit_context_decodes_strictly() {
        let mut d = cbor::Decoder::new(CHUNKED);
        assert!(decode_bytes_with(&mut d, &()).is_err());
    }
}
