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

use std::{fmt, ops::Deref, sync::Arc};

use minicbor::decode;

use crate::{Block, cardano::network_block::NetworkBlock, cbor, utils::debug_bytes};

/// Cheaply cloneable block bytes
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RawBlock(Arc<[u8]>);

impl serde::Serialize for RawBlock {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        crate::utils::serde::bytes::serialize(&self.0, serializer)
    }
}

impl<'de> serde::Deserialize<'de> for RawBlock {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        crate::utils::serde::bytes::deserialize_arc(deserializer).map(Self)
    }
}

impl schemars::JsonSchema for RawBlock {
    fn schema_name() -> String {
        "RawBlock".to_string()
    }

    fn json_schema(_gen: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        crate::utils::serde::bytes::json_schema("hex-encoded block bytes")
    }

    fn is_referenceable() -> bool {
        false
    }
}

impl fmt::Debug for RawBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let preview_hex = debug_bytes(&self.0, 32);
        let total_len = self.0.len();
        write!(f, "RawBlock({total_len}, {preview_hex})")
    }
}

impl Deref for RawBlock {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<&[u8]> for RawBlock {
    fn from(bytes: &[u8]) -> Self {
        Self(Arc::from(bytes))
    }
}

impl From<Box<[u8]>> for RawBlock {
    fn from(bytes: Box<[u8]>) -> Self {
        Self(Arc::from(bytes))
    }
}

impl RawBlock {
    /// Decode the inner Block by first decoding the raw bytes as a NetworkBlock (which contains the
    /// era tag), then by decoding the Block.
    pub fn decode(&self) -> Result<Block, decode::Error> {
        let network_block: NetworkBlock = minicbor::decode(&self.0)?;
        network_block.decode_block()
    }

    /// Decode only the header from the stored network-block bytes.
    pub fn decode_header(&self) -> Result<crate::Header, decode::Error> {
        let network_block: NetworkBlock = minicbor::decode(&self.0)?;
        network_block.decode_header()
    }
}

/// Extract the CBOR-encoded block header bytes from a raw block.
///
/// The expected structure is `[era_tag, [header_cbor, ...], ...]`.
pub fn extract_block_header_cbor(input: &[u8]) -> Result<&[u8], cbor::decode::Error> {
    let mut dec = cbor::Decoder::new(input);
    dec.array()?;
    dec.u8()?;
    dec.array()?;
    let start = dec.position();
    dec.skip()?;
    let end = dec.position();
    Ok(&input[start..end])
}

#[cfg(test)]
mod tests {
    #[test]
    fn json_is_hex_string_and_cbor_is_byte_string() {
        let payload = [0x82u8, 0x07, 0x85];
        crate::utils::serde::bytes::assert_json_hex_and_cbor_bstr(&super::RawBlock::from(payload.as_slice()), &payload);
    }
}
