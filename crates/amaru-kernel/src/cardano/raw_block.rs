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

use crate::Block;
use crate::cardano::network_block::NetworkBlock;
use minicbor::decode;
use std::{fmt, ops::Deref, sync::Arc};

/// Cheaply cloneable block bytes
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub struct RawBlock(Arc<[u8]>);

impl fmt::Debug for RawBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = &self.0;
        let total_len = bytes.len();
        let preview_len = 32.min(total_len);
        let preview = &bytes[0..preview_len];

        let mut preview_hex = String::with_capacity(2 * preview_len + 3);
        for &b in preview {
            const HEX_CHARS: [u8; 16] = *b"0123456789abcdef";
            preview_hex.push(HEX_CHARS[(b >> 4) as usize] as char);
            preview_hex.push(HEX_CHARS[(b & 0x0f) as usize] as char);
        }
        if preview_len < total_len {
            preview_hex.push_str("...");
        }

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
}

#[cfg(test)]
mod tests {
    use crate::cardano::network_block::{NetworkBlock, make_block_with_header};
    use crate::{BlockHeader, TESTNET_ERA_HISTORY, make_header};
    use amaru_minicbor_extra::{from_cbor, to_cbor};

    #[test]
    fn decode_returns_inner_block() {
        let header = BlockHeader::from(make_header(1, 42, None));
        let era_history = &*TESTNET_ERA_HISTORY;

        // make a network block from a block
        let block = make_block_with_header(&header);

        // first check the round-trip encoding / decoding for a block
        assert_eq!(block, from_cbor(to_cbor(&block).as_slice()).unwrap());

        // then check that the block can be retrieved from the network block
        let network_block = NetworkBlock::new(era_history, &block).expect("make network block");
        let decoded_block = network_block
            .decode_block()
            .expect("network block should decode");
        assert_eq!(decoded_block, block);

        // finally check that the block can be retrieved from the raw block
        let raw_block = network_block.raw_block();
        let decoded_block = raw_block.decode().expect("raw block should decode");
        assert_eq!(decoded_block, block);
    }
}
