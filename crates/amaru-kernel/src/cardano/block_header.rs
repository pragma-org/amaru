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
    BlockHeight, Hasher, Header, HeaderBody, HeaderHash, IsHeader, Point, Slot, Tip,
    cbor::{self},
    size::HEADER,
};
use std::{
    cmp::Ordering,
    fmt::{self, Debug, Display, Formatter},
};

#[cfg(any(test, feature = "test-utils"))]
mod tests;

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

/// This header type encapsulates a header and its hash to avoid recomputing
#[derive(PartialEq, Eq, Clone)]
pub struct BlockHeader {
    header: Header,
    hash: HeaderHash,
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
impl serde::Serialize for BlockHeader {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(serde::Serialize)]
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

impl<'de> serde::Deserialize<'de> for BlockHeader {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
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
        self.point().cmp(&other.point())
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
        self.hash = Hasher::<{ HEADER * 8 }>::hash_cbor(&self.header);
    }

    pub fn tip(&self) -> Tip {
        Tip::new(self.point(), self.block_height())
    }
}

impl<C> cbor::Encode<C> for BlockHeader {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        self.header.encode(e, ctx)
    }
}

impl<'b, C> cbor::Decode<'b, C> for BlockHeader {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
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

impl From<&Header> for BlockHeader {
    fn from(header: &Header) -> Self {
        let hash = Point::Origin.hash();
        let mut block_header = Self {
            header: header.clone(),
            hash,
        };
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

    fn block_height(&self) -> BlockHeight {
        self.header.header_body.block_number.into()
    }

    fn slot(&self) -> Slot {
        self.header.header_body.slot.into()
    }

    fn extended_vrf_nonce_output(&self) -> Vec<u8> {
        self.header.header_body.nonce_vrf_output()
    }
}
