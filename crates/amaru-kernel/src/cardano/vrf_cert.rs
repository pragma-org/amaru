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

use crate::{Bytes, cardano::fixed_bytes::FixedBytes, cbor};

pub const VRF_PROOF: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, cbor::Encode)]
#[cbor(context_bound = "crate::cbor::HasProtocolVersion")]
pub struct VrfCert {
    #[n(0)]
    pub output: Bytes,
    #[n(1)]
    pub proof: FixedBytes<VRF_PROOF>,
}

/// Unlike most records, the ledger reads this one with `enforceSize`, which asks for the array
/// length outright. An indefinite-length encoding is rejected at every protocol version.
impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for VrfCert {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array_definite(d, 2, |d| {
            Ok(Self { output: d.decode_with(ctx)?, proof: d.decode_with(ctx)? })
        })
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;
    use crate::protocol_version::{PROTOCOL_VERSION_10, PROTOCOL_VERSION_12};

    /// `[h'00', h'00..00']`, the shortest well-formed certificate.
    fn definite() -> Vec<u8> {
        [vec![0x82, 0x41, 0x00, 0x58, VRF_PROOF as u8], vec![0x00; VRF_PROOF]].concat()
    }

    /// The same two elements in an indefinite-length array.
    fn indefinite() -> Vec<u8> {
        [vec![0x9f, 0x41, 0x00, 0x58, VRF_PROOF as u8], vec![0x00; VRF_PROOF], vec![0xff]].concat()
    }

    /// A third element appended to the definite form.
    fn too_long() -> Vec<u8> {
        let mut bytes = definite();
        bytes[0] = 0x83;
        bytes.push(0x00);
        bytes
    }

    /// The ledger reads this record with `enforceSize`, which asks for the array length outright,
    /// so indefinite-length encodings are rejected at every protocol version, as is any extra
    /// element.
    #[test_case(PROTOCOL_VERSION_10, definite()   => matches Ok(_)  ; "definite before v12")]
    #[test_case(PROTOCOL_VERSION_12, definite()   => matches Ok(_)  ; "definite from v12")]
    #[test_case(PROTOCOL_VERSION_10, indefinite() => matches Err(_) ; "indefinite before v12")]
    #[test_case(PROTOCOL_VERSION_12, indefinite() => matches Err(_) ; "indefinite from v12")]
    #[test_case(PROTOCOL_VERSION_10, too_long()   => matches Err(_) ; "three elements")]
    fn decode_requires_a_definite_two_element_array(
        mut version: crate::ProtocolVersion,
        bytes: Vec<u8>,
    ) -> Result<VrfCert, cbor::decode::Error> {
        cbor::from_cbor_no_leftovers_with(&bytes, &mut version)
    }
}
