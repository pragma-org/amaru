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

use std::ops::Deref;

use crate::{Bytes, cbor, protocol_version::PROTOCOL_VERSION_12};

pub const CHAIN_CODE_SIZE: usize = 32;

/// The chain code half of an extended Ed25519 public key, as carried by a bootstrap witness.
///
/// Any length decodes before protocol version 12; from then on, exactly [`CHAIN_CODE_SIZE`] bytes
/// are required.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
#[repr(transparent)]
pub struct ChainCode(Bytes);

impl Deref for ChainCode {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.0.as_slice()
    }
}

impl From<Vec<u8>> for ChainCode {
    fn from(bytes: Vec<u8>) -> Self {
        Self(Bytes::from(bytes))
    }
}

impl<C> cbor::Encode<C> for ChainCode {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.encode_with(&self.0, ctx)?.ok()
    }
}

impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for ChainCode {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let bytes = cbor::decode_bytes_with(d, ctx)?;
        if ctx.protocol_version() >= PROTOCOL_VERSION_12 && bytes.len() != CHAIN_CODE_SIZE {
            return Err(cbor::decode::Error::message(format!(
                "chain code is expected to be {CHAIN_CODE_SIZE} bytes, got {}",
                bytes.len()
            )));
        }
        Ok(Self::from(bytes.into_owned()))
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;
    use crate::{
        ProtocolVersion, cbor,
        protocol_version::{PROTOCOL_VERSION_11, PROTOCOL_VERSION_12},
    };

    fn definite(len: u8) -> Vec<u8> {
        [vec![0x58, len], vec![0; len as usize]].concat()
    }

    fn chunked(lens: &[u8]) -> Vec<u8> {
        let chunks = lens.iter().flat_map(|len| [vec![0x40 + len], vec![0; *len as usize]].concat());
        [vec![0x5f], chunks.collect(), vec![0xff]].concat()
    }

    #[test_case(PROTOCOL_VERSION_11, definite(31) => matches Ok(_))]
    #[test_case(PROTOCOL_VERSION_11, definite(32) => matches Ok(_))]
    #[test_case(PROTOCOL_VERSION_11, definite(33) => matches Ok(_))]
    #[test_case(PROTOCOL_VERSION_11, chunked(&[16, 16]) => matches Err(_))]
    #[test_case(PROTOCOL_VERSION_12, definite(31) => matches Err(_))]
    #[test_case(PROTOCOL_VERSION_12, definite(32) => matches Ok(_))]
    #[test_case(PROTOCOL_VERSION_12, definite(33) => matches Err(_))]
    #[test_case(PROTOCOL_VERSION_12, chunked(&[16, 16]) => matches Ok(_))]
    #[test_case(PROTOCOL_VERSION_12, chunked(&[16, 15]) => matches Err(_))]
    fn decode(mut version: ProtocolVersion, bytes: Vec<u8>) -> Result<ChainCode, cbor::decode::Error> {
        cbor::from_cbor_no_leftovers_with(&bytes, &mut version)
    }

    #[test]
    fn re_encodes_as_a_single_definite_string() {
        let mut version = PROTOCOL_VERSION_12;
        let chain_code: ChainCode = cbor::from_cbor_no_leftovers_with(&chunked(&[16, 16]), &mut version).unwrap();
        assert_eq!(cbor::to_cbor_with(&chain_code, &mut version), definite(32));
    }
}
