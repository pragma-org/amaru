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

use std::{cmp::Ordering, fmt};

use anyhow::anyhow;

use crate::{
    BlockHeight, Bytes, Hasher, HeaderBody, HeaderHash, IsHeader, PoolId, Slot,
    cardano::fixed_bytes::FixedBytes,
    cbor, ed25519,
    size::{HEADER, POOL_COLD_KEY},
};

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Header {
    /// Header hash computed upon deserialisation and kept in memory to avoid needless
    /// re-serialisations.
    hash: HeaderHash,

    body: HeaderBody,

    /// Leader signature of the header body bytes.
    signature: Bytes,
}

// TODO: awkward Display format for Header
//
// The Display instance for header here is tailored to the need of the header_tree and
// data_generation tests. It is not used outside of these places, and it looks like something that
// should not exists to begin with.
//
// However, as I am writing this, I am chasing bigger problems that this instance and so, I leave
// the fixing of this as an exercise for the reader.
impl fmt::Display for Header {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format!(
            "{}. {}{}",
            self.slot(),
            self.hash,
            self.parent_hash().map(|p| format!(" ({p})")).unwrap_or_default()
        ))?;
        Ok(())
    }
}

impl fmt::Debug for Header {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Header")
            .field("hash", &hex::encode(self.hash()))
            .field("slot", &self.slot().as_u64())
            .field("height", &self.block_height().as_u64())
            .field("parent", &self.parent().map(hex::encode))
            .finish()
    }
}

impl PartialOrd for Header {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Header {
    fn cmp(&self, other: &Self) -> Ordering {
        self.point().cmp(&other.point())
    }
}

impl core::hash::Hash for Header {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.hash.hash(state);
    }
}

/// Concrete Conway-era compatible `Header` implementation.
///
/// There's no difference in headers' structure between Babbage
/// and Conway era. The idea is that we only keep concrete the header from
/// the latest era, and convert other headers on the fly when needed.
impl IsHeader for Header {
    fn hash(&self) -> HeaderHash {
        self.hash
    }

    fn parent(&self) -> Option<HeaderHash> {
        self.body().prev_hash
    }

    fn block_height(&self) -> BlockHeight {
        self.body().block_number.into()
    }

    fn slot(&self) -> Slot {
        self.body().slot.into()
    }

    fn vrf_output(&self) -> &[u8] {
        &self.body().vrf_result.output
    }
}

impl Header {
    /// Create a new Header from its constituant, recomputing the hash.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn new(body: HeaderBody, signature: Bytes) -> Self {
        use crate::{hash::ORIGIN_HASH, to_cbor};

        let mut header = Self { body, signature, hash: ORIGIN_HASH };

        header.hash = Hasher::<{ HEADER * 8 }>::hash(&to_cbor(&header));

        header
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub fn with_hash(mut self, hash: HeaderHash) -> Self {
        self.hash = hash;
        self
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub fn with_signature(mut self, signature: Bytes) -> Self {
        self.signature = signature;
        self
    }

    pub fn body(&self) -> &HeaderBody {
        &self.body
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub fn body_mut(&mut self) -> &mut HeaderBody {
        &mut self.body
    }

    pub fn signature(&self) -> &Bytes {
        &self.signature
    }

    pub fn parent_hash(&self) -> Option<HeaderHash> {
        self.body().prev_hash
    }

    pub fn vrf_proof(&self) -> &[u8] {
        &self.body().vrf_result.proof
    }

    pub fn issuer_verification_key(&self) -> &FixedBytes<32> {
        &self.body().issuer_verification_key
    }

    pub fn issuer(&self) -> Result<ed25519::VerifyingKey, anyhow::Error> {
        ed25519::VerifyingKey::try_from(&self.body().issuer_verification_key[..])
            .map_err(|e| anyhow!("cannot convert issuer_verification_key bytes to Ed25519 VerifyingKey").context(e))
    }

    pub fn pool_id(&self) -> PoolId {
        Hasher::<{ 8 * POOL_COLD_KEY }>::hash(&self.body().issuer_verification_key[..])
    }

    pub fn op_cert_seq(&self) -> u64 {
        self.body().operational_cert.operational_cert_sequence_number
    }
}

impl<C: cbor::HasProtocolVersion> cbor::Encode<C> for Header {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;
        e.encode_with(&self.body, ctx)?;
        e.encode_with(&self.signature, ctx)?;
        Ok(())
    }
}

impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for Header {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let ((body, signature), header_bytes) = cbor::tee(d, |d| {
            cbor::heterogeneous_array(d, |d, assert_len| {
                assert_len(2)?;
                let body = d.decode_with(ctx)?;
                let signature = d.decode_with(ctx)?;
                Ok((body, signature))
            })
        })?;

        let hash = Hasher::<{ HEADER * 8 }>::hash(header_bytes);

        Ok(Self { body, signature, hash })
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use std::sync::LazyLock;

    use proptest::prelude::*;

    use super::*;
    use crate::{
        Bytes, Hash, Header, OperationalCert, Point, ProtocolVersion, VrfCert,
        cardano::{
            fixed_bytes::FixedBytes,
            network_block::{EncodedTestBlock, make_block},
        },
        size::{BLOCK_BODY, HEADER},
    };

    /// Body hash and size of a test block, so seed headers are close to the blocks
    /// `EncodedTestBlock::from_seed` attaches to them. Tests that store blocks must still
    /// take the header from that encoding.
    static TEST_BLOCK_BODY: LazyLock<(Hash<BLOCK_BODY>, u64)> = LazyLock::new(|| {
        let block = make_block();
        (block.body_hash(), block.body_len())
    });

    /// Make a mostly empty Header with the given block_number, slot and previous hash
    pub fn make_header(block_number: u64, slot: u64, prev_hash: Option<HeaderHash>) -> Header {
        make_header_with_op_cert_seq(block_number, slot, prev_hash, 0)
    }

    /// Like [`make_header`] but with a configurable operational certificate sequence number,
    /// used when testing chain selection where the higher op_cert_seq chain is preferred.
    pub fn make_header_with_op_cert_seq(
        block_number: u64,
        slot: u64,
        prev_hash: Option<HeaderHash>,
        op_cert_seq: u64,
    ) -> Header {
        let (block_body_hash, block_body_size) = *TEST_BLOCK_BODY;

        Header::new(
            HeaderBody {
                block_number,
                slot,
                prev_hash,
                issuer_verification_key: FixedBytes::zeroes(),
                vrf_verification_key: FixedBytes::zeroes(),
                vrf_result: VrfCert { output: Bytes::default(), proof: FixedBytes::zeroes() },
                block_body_size,
                block_body_hash,
                operational_cert: OperationalCert {
                    operational_cert_hot_verification_key: FixedBytes::zeroes(),
                    operational_cert_sequence_number: op_cert_seq,
                    operational_cert_kes_period: 0,
                    operational_cert_sigma: FixedBytes::zeroes(),
                },
                protocol_version: ProtocolVersion::new(1, 2),
            },
            Bytes::default(),
        )
    }

    /// Create a list of arbitrary headers starting from a root, and where chain\[i\] is the parent of chain\[i+1\]
    pub fn any_headers_chain(n: usize) -> impl Strategy<Value = Vec<Header>> {
        prop::collection::vec(any_header(), n).prop_map(make_headers())
    }

    /// Create a list of arbitrary headers starting from a root with the specified hash, and where chain\[i\] is the parent of chain\[i+1\]
    pub fn any_headers_chain_with_root(n: usize, point: Point) -> impl Strategy<Value = Vec<Header>> {
        prop::collection::vec(any_header(), n).prop_map(make_headers_with_root_point(Some(point)))
    }

    /// Given a list of headers, set their block_number, slot and parent fields to form a valid chain
    fn make_headers() -> impl Fn(Vec<Header>) -> Vec<Header> {
        make_headers_with_root_point(None)
    }

    /// Given a list of headers, set their block_number, slot and parent fields to form a valid chain
    /// The returned headers increase their block number and slot by 1 at each step, starting from the given root point
    fn make_headers_with_root_point(point: Option<Point>) -> impl Fn(Vec<Header>) -> Vec<Header> {
        move |headers| {
            let mut parent = point.unwrap_or(Point::Origin);
            headers
                .into_iter()
                .map({
                    |mut header| {
                        header.body_mut().slot = (parent.slot_or_default() + 1).as_u64();
                        header.body_mut().block_number = header.body().slot;
                        header.body_mut().prev_hash = Some(parent.hash());
                        let header = EncodedTestBlock::from_seed(&header, &crate::EraHistory::default()).header;
                        parent = header.point();
                        header
                    }
                })
                .collect()
        }
    }

    /// Create an arbitrary Header, with an arbitrary parent, possibly set to None
    pub fn any_header() -> impl Strategy<Value = Header> {
        (0u64..=1_000_000, 0u64..=1_000_000, prop::option::weighted(0.01, any_header_hash()))
            .prop_map(|(block_number, slot, prev_hash)| make_header(block_number, slot, prev_hash))
    }

    /// Create an arbitrary Header, with an arbitrary parent
    pub fn any_header_with_parent(parent: HeaderHash) -> impl Strategy<Value = Header> {
        (0u64..=1_000_000, 0u64..=1_000_000)
            .prop_map(move |(block_number, slot)| make_header(block_number, slot, Some(parent)))
    }

    /// Create an arbitrary Header, with an arbitrary parent that is guaranteed to be Some
    pub fn any_header_with_some_parent() -> impl Strategy<Value = Header> {
        any_header().prop_flat_map(|h| any_header_with_parent(h.hash()))
    }

    /// Create an arbitrary header hash with the right number of bytes
    pub fn any_header_hash() -> impl Strategy<Value = HeaderHash> {
        any::<[u8; HEADER]>().prop_map(Hash::from)
    }

    /// Create an arbitrary FakeHeader
    pub fn any_fake_header() -> impl Strategy<Value = Header> {
        (0u64..=1_000_000, 0u64..=1_000_000, prop::option::weighted(0.01, any_header_hash()))
            .prop_map(|(block_number, slot, parent)| make_header(block_number, slot, parent))
    }
}
