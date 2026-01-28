// Copyright 2025 PRAGMA
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

use crate::{
    HEADER_HASH_SIZE, Hasher, Header, HeaderBody, HeaderHash, IsHeader, Point,
    cbor::{self, Decode, Decoder, Encode, Encoder, encode::Write},
    protocol_messages::tip::Tip,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
    cmp::Ordering,
    fmt::{self, Debug, Display, Formatter},
};

/// This header type encapsulates a header and its hash to avoid recomputing
#[derive(PartialEq, Eq, Clone)]
pub struct BlockHeader {
    pub(super) header: Header,
    pub(super) hash: HeaderHash,
}

impl Display for BlockHeader {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&format!(
            "{}. {}{}",
            self.slot(),
            self.hash(),
            self.parent_hash()
                .map(|p| format!(" ({p})"))
                .unwrap_or_default()
        ))?;
        Ok(())
    }
}

/// We serialize both the hash and the header, but we use serde's flattening
/// to avoid nesting the header inside another object.
impl Serialize for BlockHeader {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct BlockHeaderSer<'a> {
            hash: &'a HeaderHash,
            #[serde(flatten)]
            header: &'a Header,
        }

        let helper = BlockHeaderSer {
            hash: &self.hash,
            header: &self.header,
        };

        helper.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for BlockHeader {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let header = Header::deserialize(deserializer)?;
        Ok(BlockHeader::from(header))
    }
}

impl Debug for BlockHeader {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlockHeader")
            .field("hash", &hex::encode(self.hash()))
            .field("slot", &self.slot().as_u64())
            .field("parent", &self.parent().map(hex::encode))
            .finish()
    }
}

impl PartialOrd for BlockHeader {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BlockHeader {
    fn cmp(&self, other: &Self) -> Ordering {
        self.hash.cmp(&other.hash())
    }
}

impl core::hash::Hash for BlockHeader {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.hash.hash(state);
    }
}

impl BlockHeader {
    /// Create a new BlockHeader from a Header and its precomputed hash
    /// Note: The hash is not verified to match the header!
    #[cfg(feature = "test-utils")]
    pub fn new(header: Header, hash: HeaderHash) -> Self {
        Self { header, hash }
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn header_body(&self) -> &HeaderBody {
        &self.header.header_body
    }

    pub fn parent_hash(&self) -> Option<HeaderHash> {
        self.header.header_body.prev_hash
    }

    fn recompute_hash(&mut self) {
        self.hash = Hasher::<{ HEADER_HASH_SIZE * 8 }>::hash_cbor(&self.header);
    }

    pub fn tip(&self) -> Tip {
        Tip::new(self.point(), self.block_height())
    }

    /// Extract header from a block in stored format: [header, tx_bodies, witnesses, ...]
    ///
    /// This is the format used when blocks are stored in the database.
    pub fn extract_header_from_stored_block(
        block_bytes: &[u8],
    ) -> Result<BlockHeader, cbor::decode::Error> {
        let mut decoder = Decoder::new(block_bytes);

        // Stored format: [header, tx_bodies, witnesses, auxiliary_data?, invalid_transactions?]
        decoder.array()?;

        // Decode the header (this is the pallas Header type wrapped in KeepRaw)
        let header: Header = decoder.decode()?;
        Ok(BlockHeader::from(header))
    }

    /// Extract header from a block in network format: [era_tag, [header, tx_bodies, witnesses, ...]]
    ///
    /// This is the format used when blocks are fetched from Cardano nodes over the network.
    /// This function handles the era tag wrapper and then decodes the header from the inner MintedBlock.
    ///
    /// For blocks in stored format (without era tag), use `extract_header_from_stored_block` instead.
    pub fn extract_header(block_bytes: &[u8]) -> Result<BlockHeader, cbor::decode::Error> {
        let mut decoder = Decoder::new(block_bytes);

        // Network format: [era_tag, MintedBlock]
        // Read the outer array (should be length 2)
        decoder.array()?;

        // Skip the era tag (u16)
        decoder.u16()?;

        // Read the inner MintedBlock array
        decoder.array()?;

        // Now decode the header from the inner array
        let header: Header = decoder.decode()?;
        Ok(BlockHeader::from(header))
    }
}

impl<C> Encode<C> for BlockHeader {
    fn encode<W: Write>(
        &self,
        e: &mut Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        self.header.encode(e, ctx)
    }
}

impl<'b, C> Decode<'b, C> for BlockHeader {
    fn decode(d: &mut Decoder<'b>, ctx: &mut C) -> Result<Self, minicbor::decode::Error> {
        let header = Header::decode(d, ctx)?;
        Ok(BlockHeader::from(header))
    }
}

impl From<Header> for BlockHeader {
    fn from(header: Header) -> Self {
        let hash = Point::Origin.hash();
        let mut block_header = Self { header, hash };
        block_header.recompute_hash();
        block_header
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::is_header::tests::make_header;

    #[test]
    fn test_extract_header_from_stored_format_block() {
        let header = make_header(42u64, 100u64, Some(HeaderHash::from([1u8; 32])));
        let expected_header = BlockHeader::from(header.clone());

        let block_bytes = make_stored_block(&header);
        let extracted_header = BlockHeader::extract_header_from_stored_block(&block_bytes)
            .expect("failed to extract header from stored format block");

        assert_eq!(&extracted_header, &expected_header);
    }

    #[test]
    fn test_extract_header_from_network_format_block() {
        let header = make_header(42u64, 100u64, Some(HeaderHash::from([1u8; 32])));
        let expected_header = BlockHeader::from(header.clone());

        let stored_block = make_stored_block(&header);
        let block_bytes = make_network_block(&stored_block);

        let extracted_header = BlockHeader::extract_header(&block_bytes)
            .expect("failed to extract header from network format block");

        assert_eq!(&extracted_header, &expected_header);
    }

    #[test]
    fn test_extract_header_from_invalid_block() {
        // Test with invalid CBOR data
        let invalid_bytes = vec![0xFF, 0xFF, 0xFF];
        let result = BlockHeader::extract_header(&invalid_bytes);
        assert!(result.is_err(), "should fail on invalid CBOR");
    }

    #[test]
    fn test_extract_header_from_empty_block() {
        // Test with empty bytes
        let empty_bytes = vec![];
        let result = BlockHeader::extract_header(&empty_bytes);
        assert!(result.is_err(), "should fail on empty bytes");
    }

    // HELPERS

    /// Create a stored format block from a header.
    /// Format: [header, tx_bodies, witnesses, auxiliary_data?, invalid_transactions?]
    fn make_stored_block(header: &Header) -> Vec<u8> {
        let mut block_bytes = Vec::new();
        let mut encoder = cbor::Encoder::new(&mut block_bytes);

        encoder.array(5).expect("failed to encode array");
        encoder.encode(header).expect("failed to encode header");
        encoder.array(0).expect("failed to encode tx bodies");
        encoder.array(0).expect("failed to encode witnesses");
        encoder.null().expect("failed to encode auxiliary data");
        encoder.null().expect("failed to encode invalid txs");

        block_bytes
    }

    /// Create a network format block from a stored format block.
    /// Format: [era_tag, stored_block]
    fn make_network_block(stored_block: &[u8]) -> Vec<u8> {
        use crate::CONWAY_ERA_TAG;
        let mut block_bytes = Vec::new();
        let mut encoder = cbor::Encoder::new(&mut block_bytes);

        encoder.array(2).expect("failed to encode outer array");
        encoder
            .u16(CONWAY_ERA_TAG)
            .expect("failed to encode era tag");

        // Write the stored block bytes directly
        block_bytes.extend_from_slice(stored_block);

        block_bytes
    }
}
