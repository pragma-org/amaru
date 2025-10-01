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

use super::*;
use amaru_kernel::cbor;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::Debug;
use std::{
    fmt,
    fmt::{Display, Formatter},
};

/// Basic `Header` implementation for testing purposes.
#[derive(PartialEq, Eq, Clone, Copy, PartialOrd, Ord)]
pub struct FakeHeader {
    pub block_number: u64,
    pub slot: u64,
    pub parent: Option<Hash<HEADER_HASH_SIZE>>,
    pub body_hash: Hash<HEADER_HASH_SIZE>,
}

// Manual serde implementation for FakeHeader
impl Serialize for FakeHeader {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("FakeHeader", 4)?;
        state.serialize_field("block_number", &self.block_number)?;
        state.serialize_field("slot", &self.slot)?;
        state.serialize_field(
            "parent",
            &self.parent.as_ref().map(|h| hex::encode(h.as_ref())),
        )?;
        state.serialize_field("body_hash", &hex::encode(self.body_hash.as_ref()))?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for FakeHeader {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct FakeHeaderHelper {
            block_number: u64,
            slot: u64,
            parent: Option<String>,
            body_hash: String,
        }

        let helper = FakeHeaderHelper::deserialize(deserializer)?;

        let parent = if let Some(parent_str) = helper.parent {
            let bytes = hex::decode(&parent_str).map_err(serde::de::Error::custom)?;
            let mut arr = [0u8; HEADER_HASH_SIZE];
            arr.copy_from_slice(&bytes);
            Some(Hash::from(arr))
        } else {
            None
        };

        let body_hash_bytes = hex::decode(&helper.body_hash).map_err(serde::de::Error::custom)?;
        let mut body_hash_arr = [0u8; HEADER_HASH_SIZE];
        body_hash_arr.copy_from_slice(&body_hash_bytes);
        let body_hash = Hash::from(body_hash_arr);

        Ok(FakeHeader {
            block_number: helper.block_number,
            slot: helper.slot,
            parent,
            body_hash,
        })
    }
}

impl Debug for FakeHeader {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakeHeader")
            .field("block_number", &self.block_number)
            .field("slot", &self.slot)
            .field("hash", &self.hash().to_string())
            .field("body", &self.body_hash.to_string())
            .finish()
    }
}

impl IsHeader for FakeHeader {
    fn parent(&self) -> Option<Hash<HEADER_HASH_SIZE>> {
        self.parent
    }

    fn block_height(&self) -> u64 {
        self.block_number
    }

    fn slot(&self) -> u64 {
        self.slot
    }

    fn point(&self) -> Point {
        Point::Specific(self.slot(), self.hash().to_vec())
    }

    fn extended_vrf_nonce_output(&self) -> Vec<u8> {
        unimplemented!(
            "called 'extended_vrf_nonce_output' on a Fake header clearly not ready for that."
        )
    }
}

impl Display for FakeHeader {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "FakeHeader {{ hash: {}, block_number: {}, slot: {}, parent: {}, body_hash: {} }}",
            self.hash(),
            self.block_number,
            self.slot,
            self.parent
                .map(|h| h.to_string())
                .unwrap_or_else(|| "None".to_string()),
            self.body_hash
        )
    }
}

impl<C> cbor::encode::Encode<C> for FakeHeader {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(4)?
            .encode_with(self.block_number, ctx)?
            .encode_with(self.slot, ctx)?
            .encode_with(self.parent, ctx)?
            .encode_with(self.body_hash, ctx)?
            .ok()
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for FakeHeader {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        let block_number = d.decode_with(ctx)?;
        let slot = d.decode_with(ctx)?;
        let parent = d.decode_with(ctx)?;
        let body_hash = d.decode_with(ctx)?;
        Ok(Self {
            block_number,
            slot,
            parent,
            body_hash,
        })
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use super::*;
    use amaru_kernel::{Bytes, HeaderBody, PseudoHeader};
    use pallas_primitives::VrfCert;
    use pallas_primitives::babbage::OperationalCert;
    use proptest::prelude::*;
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    /// Create a list of arbitrary fake headers starting from a root, and where chain[i] is the parent of chain[i+1]
    pub fn any_fake_headers_chain(n: usize) -> impl Strategy<Value = Vec<FakeHeader>> {
        prop::collection::vec(any_fake_header(), n).prop_map(make_fake_header())
    }

    fn make_fake_header() -> impl Fn(Vec<FakeHeader>) -> Vec<FakeHeader> {
        |mut headers| {
            let mut parent = None;
            for (i, h) in headers.iter_mut().enumerate() {
                h.block_number = i as u64;
                h.slot = i as u64;
                h.parent = parent;
                parent = Some(h.hash())
            }
            headers
        }
    }

    /// Create an arbitrary header hash with the right number of bytes
    pub fn any_header_hash() -> impl Strategy<Value = Hash<HEADER_HASH_SIZE>> {
        any::<[u8; HEADER_HASH_SIZE]>().prop_map(Hash::from)
    }

    /// Create an arbitrary FakeHeader
    pub fn any_fake_header() -> impl Strategy<Value = FakeHeader> {
        (
            0u64..=1_000_000,
            0u64..=1_000_000,
            prop::option::weighted(0.01, any_header_hash()),
            any_header_hash(),
        )
            .prop_map(|(block_number, slot, parent, body_hash)| FakeHeader {
                block_number,
                slot,
                parent,
                body_hash,
            })
    }

    /// Create a list of arbitrary headers starting from a root, and where chain[i] is the parent of chain[i+1]
    pub fn any_headers_chain(n: usize) -> impl Strategy<Value = Vec<Header>> {
        prop::collection::vec(any_header(), n).prop_map(make_header())
    }

    fn make_header() -> impl Fn(Vec<Header>) -> Vec<Header> {
        |mut headers| {
            let mut parent = None;
            for (i, h) in headers.iter_mut().enumerate() {
                h.header_body.block_number = i as u64;
                h.header_body.slot = i as u64;
                h.header_body.prev_hash = parent;
                parent = Some(h.hash())
            }
            headers
        }
    }

    pub fn any_header() -> impl Strategy<Value = Header> {
        (
            0u64..=1_000_000,
            0u64..=1_000_000,
            prop::option::weighted(0.01, any_header_hash()),
        )
            .prop_map(|(block_number, slot, prev_hash)| PseudoHeader {
                header_body: HeaderBody {
                    block_number,
                    slot,
                    prev_hash,
                    issuer_vkey: Bytes::from(vec![]),
                    vrf_vkey: Bytes::from(vec![]),
                    vrf_result: VrfCert(Bytes::from(vec![]), Bytes::from(vec![])),
                    block_body_size: 0,
                    block_body_hash: Hash::<32>::from([0u8; 32]),
                    operational_cert: OperationalCert {
                        operational_cert_hot_vkey: Bytes::from(vec![]),
                        operational_cert_sequence_number: 0,
                        operational_cert_kes_period: 0,
                        operational_cert_sigma: Bytes::from(vec![]),
                    },
                    protocol_version: (1, 2),
                },
                body_signature: Bytes::from(vec![]),
            })
    }

    /// Run a strategy and return the generated value, panicking if generation fails.
    #[expect(clippy::unwrap_used)]
    pub fn run<T>(s: impl Strategy<Value = T>) -> T {
        let mut runner = TestRunner::default();
        s.new_tree(&mut runner).unwrap().current()
    }
}
