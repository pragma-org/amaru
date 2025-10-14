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

use amaru_kernel::cbor::encode::Write;
use amaru_kernel::cbor::{Decode, Decoder, Encode, Encoder};
use amaru_kernel::{HEADER_HASH_SIZE, Hash, Hasher, Header, HeaderBody, MintedHeader, Point, cbor};
use serde::Serializer;
use serde::{Deserialize, Deserializer, Serialize};
use std::cmp::Ordering;
use std::fmt;
use std::fmt::{Debug, Display, Formatter};

#[cfg(any(test, feature = "test-utils"))]
pub mod tests;

/// Interface to a header for the purpose of chain selection.
pub trait IsHeader: cbor::Encode<()> + Sized {
    /// Hash of the header
    ///
    /// This is used to identify the header in the chain selection.
    /// Header hash is expected to be unique for each header, eg.
    /// $h \neq h' \logeq hhash() \new h'.hash()$.
    fn hash(&self) -> Hash<HEADER_HASH_SIZE>;

    /// Point to this header
    fn point(&self) -> Point {
        Point::Specific(self.slot(), self.hash().to_vec())
    }

    /// Parent hash of the header
    /// Not all headers have a parent, eg. genesis block.
    fn parent(&self) -> Option<Hash<HEADER_HASH_SIZE>>;

    /// Block height of the header w.r.t genesis block
    fn block_height(&self) -> u64;

    /// Slot number of the header
    fn slot(&self) -> u64;

    /// The range-extended tagged nonce vrf output
    // TODO: Return type here should be a Hash<32>, but we cannot make this happen without either:
    // 1. Making this return a Result
    // 2. Use a panic
    // 3. Fix Pallas' leader_vrf_output to return a Hash<32> instead of a Vec.
    fn extended_vrf_nonce_output(&self) -> Vec<u8>;
}

/// Type alias for a header hash to improve readability
pub type HeaderHash = Hash<HEADER_HASH_SIZE>;

/// This header type encapsulates a header and its hash to avoid recomputing
#[derive(PartialEq, Eq, Clone)]
pub struct BlockHeader {
    header: Header,
    hash: HeaderHash,
}

impl Serialize for BlockHeader {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.header.serialize(serializer)
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
            .field("slot", &self.slot())
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

impl Display for BlockHeader {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
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

    fn recompute_hash(&mut self) {
        self.hash = Hasher::<{ HEADER_HASH_SIZE * 8 }>::hash_cbor(&self.header);
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

/// Concrete Conway-era compatible `Header` implementation.
///
/// There's no difference in headers' structure between Babbage
/// and Conway era. The idea is that we only keep concrete the header from
/// the latest era, and convert other headers on the fly when needed.
impl IsHeader for BlockHeader {
    fn hash(&self) -> HeaderHash {
        self.hash
    }

    fn parent(&self) -> Option<HeaderHash> {
        self.header.header_body.prev_hash
    }

    fn block_height(&self) -> u64 {
        self.header.header_body.block_number
    }

    fn slot(&self) -> u64 {
        self.header.header_body.slot
    }

    fn extended_vrf_nonce_output(&self) -> Vec<u8> {
        self.header.header_body.nonce_vrf_output()
    }
}

impl IsHeader for MintedHeader<'_> {
    fn hash(&self) -> Hash<HEADER_HASH_SIZE> {
        Hasher::<{ HEADER_HASH_SIZE * 8 }>::hash_cbor(&self)
    }

    fn parent(&self) -> Option<Hash<HEADER_HASH_SIZE>> {
        self.header_body.prev_hash
    }

    fn block_height(&self) -> u64 {
        self.header_body.block_number
    }

    fn slot(&self) -> u64 {
        self.header_body.slot
    }

    fn extended_vrf_nonce_output(&self) -> Vec<u8> {
        self.header_body.nonce_vrf_output()
    }
}
