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

use crate::echo::Envelope;
use crate::simulator::bytes::Bytes;
use amaru_consensus::consensus::events::ChainSyncEvent;
use amaru_kernel::peer::Peer;
use amaru_kernel::{HeaderHash, Point, cbor};
use amaru_ouroboros_traits::BlockHeader;
use amaru_slot_arithmetic::Slot;
use gasket::framework::WorkerError;
use pallas_primitives::babbage::{Header, MintedHeader};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fmt::{Display, Formatter};
use tracing::Span;

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChainSyncMessage {
    Init {
        msg_id: u64,
        node_id: String,
        node_ids: Vec<String>,
    },
    InitOk {
        in_reply_to: u64,
    },
    Fwd {
        msg_id: u64,
        slot: Slot,
        hash: Bytes,
        header: Bytes,
    },
    Bck {
        msg_id: u64,
        slot: Slot,
        hash: Bytes,
    },
}

impl ChainSyncMessage {
    /// Attempt to decode the block header from a `Fwd` message.
    pub fn decode_block_header(&self) -> Option<BlockHeader> {
        match self {
            ChainSyncMessage::Fwd { header, .. } => {
                if let Ok(minted_header) = cbor::decode::<MintedHeader<'_>>(&header.bytes) {
                    Some(BlockHeader::from(Header::from(minted_header)))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Attempt to decode a header hash from a `Fwd` or `Bck` message.
    pub fn header_hash(&self) -> Option<HeaderHash> {
        match self {
            ChainSyncMessage::Fwd { hash, .. } => Some(HeaderHash::from(hash.bytes.as_slice())),
            ChainSyncMessage::Bck { hash, .. } => Some(HeaderHash::from(hash.bytes.as_slice())),
            _ => None,
        }
    }
}

impl fmt::Debug for ChainSyncMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChainSyncMessage::Init {
                msg_id,
                node_id,
                node_ids,
            } => f
                .debug_struct("Init")
                .field("msg_id", msg_id)
                .field("node_id", node_id)
                .field("node_ids", node_ids)
                .finish(),
            ChainSyncMessage::InitOk { in_reply_to } => f
                .debug_struct("InitOk")
                .field("in_reply_to", in_reply_to)
                .finish(),
            msg @ ChainSyncMessage::Fwd {
                msg_id,
                slot,
                hash,
                header,
            } => {
                let parent_hash = msg
                    .decode_block_header()
                    .and_then(|h| {
                        h.parent_hash().map(|h| {
                            let mut s = h.to_string();
                            s.truncate(6);
                            s
                        })
                    })
                    .unwrap_or("n/a".to_string());
                f.debug_struct("Fwd")
                    .field("msg_id", msg_id)
                    .field("slot", slot)
                    .field("hash", &hex::encode(&hash.bytes[..hash.bytes.len().min(3)]))
                    .field("parent_hash", &parent_hash)
                    .field(
                        "header",
                        &hex::encode(&header.bytes.as_slice()[..header.bytes.len().min(4)]),
                    )
                    .finish()
            }
            ChainSyncMessage::Bck { msg_id, slot, hash } => f
                .debug_struct("Bck")
                .field("msg_id", msg_id)
                .field("slot", slot)
                .field(
                    "hash",
                    &hex::encode(&hash.bytes.as_slice()[..hash.bytes.len().min(3)]),
                )
                .finish(),
        }
    }
}

impl Display for ChainSyncMessage {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ChainSyncMessage::Init {
                msg_id,
                node_id,
                node_ids,
            } => write!(
                f,
                "Init ({}) node_id={}, node_ids=[{}]",
                msg_id,
                node_id,
                node_ids.join(", ")
            ),
            ChainSyncMessage::InitOk { in_reply_to } => {
                write!(f, "InitOk ({})", in_reply_to)
            }
            ChainSyncMessage::Fwd {
                msg_id,
                slot,
                hash,
                header: _,
            } => write!(
                f,
                "Forward ({}) {}/{}",
                msg_id,
                slot,
                hex::encode(&hash.bytes.as_slice()[..hash.bytes.len().min(3)])
            ),
            ChainSyncMessage::Bck { msg_id, slot, hash } => write!(
                f,
                "Backward ({}) {}/{}",
                msg_id,
                slot,
                hex::encode(&hash.bytes.as_slice()[..hash.bytes.len().min(3)])
            ),
        }
    }
}

impl Envelope<ChainSyncMessage> {
    pub fn to_chain_sync_event(self, span: Span) -> Result<ChainSyncEvent, WorkerError> {
        use ChainSyncMessage::*;
        let peer = Peer { name: self.src };

        match self.body {
            Fwd {
                msg_id: _,
                slot,
                hash,
                header,
            } => Ok(ChainSyncEvent::RollForward {
                peer,
                point: Point::Specific((slot).into(), hash.into()),
                raw_header: header.into(),
                span,
            }),
            Bck {
                msg_id: _,
                slot,
                hash,
            } => Ok(ChainSyncEvent::Rollback {
                peer,
                rollback_point: Point::Specific(slot.into(), hash.into()),
                span,
            }),
            _ => Err(WorkerError::Recv),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::sync::ChainSyncMessage::{Bck, Fwd};
    use amaru_kernel::{HeaderHash, cbor};
    use pallas_crypto::hash::{Hash, Hasher};
    use pallas_primitives::babbage;
    use proptest::prelude::BoxedStrategy;
    use proptest::proptest;
    use std::str::FromStr;

    proptest! {
        #[test]
        fn roundtrip_chain_sync_messages(message in arbitrary_message()) {
            let encoded = serde_json::to_string(&message).unwrap();
            let decoded = serde_json::from_str(&encoded).unwrap();
            assert_eq!(message, decoded);
        }
    }

    #[test]
    fn can_retrieve_forward_from_message() {
        let fwd = some_forward();
        let expected_hash: HeaderHash = HeaderHash::from_str(
            "746353a52e80b3ac2d6d51658df7988d4b7baa219f55584a4827bd00bc97617e",
        )
        .unwrap();
        let message = Envelope {
            src: "peer1".to_string(),
            dest: "me".to_string(),
            body: fwd,
        };

        let event = message
            .to_chain_sync_event(tracing::trace_span!("test"))
            .unwrap();

        match event {
            ChainSyncEvent::RollForward {
                peer,
                point,
                raw_header,
                ..
            } => {
                assert_eq!(peer.name, "peer1");
                assert_eq!(point.slot_or_default(), Slot::from(1234));
                assert_eq!(Hash::from(&point), expected_hash);
                assert_eq!(raw_header, hex::decode(TEST_HEADER).unwrap());
            }
            _ => panic!("expected RollForward event"),
        }
    }

    // HELPERS

    const TEST_HEADER: &str = "828a1b7f18687338385c451b7f3f6a46c620f2bf58201ef19c82fd7039353d2d027e0adcd1588674a1f67673e2ad8898e8ad3a959bba58204346f1024dc69b1c976350d9468792faea3e0f6f8cfcb73a77a9e02ca4ef26f758205ef67af0ce8fd5c7528f2dc014ebbd99ec956a23e4242d8d5425fa488740a98d8258405d3f1a0b3a6516afeba663d58507a47fe8501e4921c2d5363ec3d883678c4c097e06db41d8a560a16055f5115b32379594f99d7db5a32957be678a55b2dd9fda5850ea7a1649b1638fab16565fcdabfd7c89ae6359c422677e24463e7a122237640ffc1f20263b0d1a6e2b9dd2d0b3331df2bd6f4519e96e2d60d8fc88f65c4e403b06da57210ff07d9a8268ad68bb4206001a00013df0582017d93bc9a012ff78060f4080484caea4c31a7381b8cb9208dac53657e4a902938458200f79db338e0ade358ab6d910e14260fc1f2065fc1dbba49c821653816258e56518491b00128fbdd0d9f26d5840d4c013511cbce3c3bf8dd2ef4eac37b3cb5492f1f8ab6f80897c2286c9f7f7e545d045781ad72b6d7196798159fc7241dff87ae9f9dd29f73fe7aad24f49c8008200005901c0129bc460199bc83dc6eaee50d0bbf89f4c43a91c6fd5ff25f181dc31a66420d276821f40d7801eca552684174320483e0ac28eb99f7317695345f6f9d71bb208bbff4ee78d8743d04569833373dfc682cae0bf2f110a90164f8e1e19ed12e36b1fc0884d9e81ae7533b608255fa8a694b1c3da5388cc427b9ade76a3f27892b813789eb814764587eb40c3c5d525fe24d2a3aa3fe3839dbd447dc606689393275b257d8e773878bf40ceed082c3f90fe3e70332322a0d84a57c1f61f7cb2a99920de88e2ab86f3b11b5ae756055718ce817511042c4826fbc6254a42617857323e73abbc7f33303879facc9ccf454820e31d3a271ac4435e6b7ae17c1b78292fedb5ee4c92aa87f797921c71ab626a5e44871761d332c606331b604bb3f069663d7750ad2868d2d9ad80d3bc5a83ce63911dc53395861ae296ec9fe14e2d0c8cbf3402c7d4b2b11107444e7dad3b3f0ce78860baea60ecd02aaab366b6395daa9d1bb1bf80599bfde25c233608c6860a0d7ae51f8487ba3f77e0e2ba5a452c19ae46055b0e6062e6647277b01dca68ba74cc32ced883f26965c551f20ed2a893173bec9eb216efefdffd360a2ed166d82fe3263f826041650d5cc567399f0c9c";

    fn some_forward() -> ChainSyncMessage {
        let header_bytes = hex::decode(TEST_HEADER).unwrap();
        let header: babbage::MintedHeader<'_> = cbor::decode(&header_bytes).unwrap();
        let header_hash = Hasher::<256>::hash(header.raw_cbor());
        Fwd {
            msg_id: 1,
            slot: Slot::from(1234),
            hash: header_hash.to_vec().into(),
            header: header_bytes.into(),
        }
    }

    fn arbitrary_message() -> BoxedStrategy<ChainSyncMessage> {
        use proptest::{collection::vec, prelude::*};

        prop_oneof![
            (any::<u64>(), any::<String>(), vec(any::<String>(), 0..10)).prop_map(
                |(msg_id, node_id, node_ids)| ChainSyncMessage::Init {
                    msg_id,
                    node_id,
                    node_ids
                }
            ),
            (any::<u64>()).prop_map(|msg_id| ChainSyncMessage::InitOk {
                in_reply_to: msg_id
            }),
            (any::<u64>(), any::<u64>(), any::<[u8; 32]>()).prop_map(|(msg_id, slot, hash)| {
                Fwd {
                    msg_id,
                    slot: Slot::from(slot),
                    hash: hash.to_vec().into(),
                    header: Bytes::new(),
                }
            }),
            (any::<u64>(), any::<u64>(), any::<[u8; 32]>()).prop_map(|(msg_id, slot, hash)| Bck {
                msg_id,
                slot: Slot::from(slot),
                hash: hash.to_vec().into()
            })
        ]
        .boxed()
    }
}
