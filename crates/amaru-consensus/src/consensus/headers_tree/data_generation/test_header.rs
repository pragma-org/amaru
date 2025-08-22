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

use amaru_kernel::{cbor, Point, HEADER_HASH_SIZE};
use amaru_ouroboros_traits::IsHeader;
use pallas_crypto::hash::Hash;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::fmt::{Debug, Display, Formatter};
use std::hash::Hasher;

/// Simplified version of a header
/// It essentially keeps track only of the parent->child relationship between headers and the header slot.
///
/// This is more practical to operate than a FakeHeader where the hash is computed on the whole header serialized data
/// which is harder to control for tests.
///
#[derive(PartialEq, Eq, Clone, Copy, PartialOrd, Ord, Deserialize, Serialize)]
pub struct TestHeader {
    pub hash: Hash<HEADER_HASH_SIZE>,
    pub slot: u64,
    pub parent: Option<Hash<HEADER_HASH_SIZE>>,
}

#[derive(PartialEq, Eq, Clone, Copy, PartialOrd, Ord)]
struct TestHash(Hash<HEADER_HASH_SIZE>);

impl<'de> Deserialize<'de> for TestHash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let buf = <String>::deserialize(deserializer)?;
        let bytes = hex::decode(buf).map_err(serde::de::Error::custom)?;
        Ok(TestHash(Hash::from(bytes.as_slice())))
    }
}

impl Serialize for TestHash {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl std::hash::Hash for TestHeader {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write(self.hash.as_slice())
    }
}

impl Debug for TestHeader {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("TestHeader")
            .field("hash", &self.hash.to_string())
            .field("slot", &self.slot.to_string())
            .field(
                "parent",
                &self
                    .parent
                    .map(|h| h.to_string())
                    .unwrap_or("None".to_string()),
            )
            .finish()
    }
}

impl Display for TestHeader {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&format!(
            "{} (slot: {}, parent: {})",
            self.hash,
            self.slot,
            self.parent
                .map(|p| p.to_string())
                .unwrap_or("None".to_string())
        ))
    }
}

impl IsHeader for TestHeader {
    fn hash(&self) -> Hash<HEADER_HASH_SIZE> {
        self.hash
    }

    fn point(&self) -> Point {
        Point::Specific(self.slot(), self.hash.to_vec())
    }

    fn parent(&self) -> Option<Hash<HEADER_HASH_SIZE>> {
        self.parent
    }

    fn block_height(&self) -> u64 {
        0
    }

    fn slot(&self) -> u64 {
        self.slot
    }

    fn extended_vrf_nonce_output(&self) -> Vec<u8> {
        unimplemented!(
            "called 'extended_vrf_nonce_output' on a TestHeader clearly not ready for that."
        )
    }
}

impl<C> cbor::encode::Encode<C> for TestHeader {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(3)?
            .encode_with(self.hash, ctx)?
            .encode_with(self.slot, ctx)?
            .encode_with(self.parent, ctx)?
            .ok()
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for TestHeader {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        let hash = d.decode_with(ctx)?;
        let slot = d.decode_with(ctx)?;
        let parent = d.decode_with(ctx)?;
        Ok(Self { hash, slot, parent })
    }
}

mod tests {
    use super::*;
    use amaru_kernel::{from_cbor, to_cbor};
    use proptest::prelude::any;
    use proptest::{prop_compose, proptest};

    proptest! {
        #[test]
        fn prop_roundtrip_cbor(hdr in any_test_header()) {
            let bytes = to_cbor(&hdr);
            let hdr2 = from_cbor::<TestHeader>(&bytes).unwrap();
            assert_eq!(hdr, hdr2);
        }
    }

    prop_compose! {
        pub fn any_test_header()(
            slot in 0..1000000u64,
            parent in any::<[u8; 32]>(),
            body in any::<[u8; 32]>(),
        )
            -> TestHeader {
            TestHeader {
                hash: body.into(),
                slot,
                parent: Some(parent.into()),
            }
        }
    }
}
