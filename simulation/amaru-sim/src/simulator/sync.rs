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

use super::bytes::Bytes;
use crate::echo::Envelope;
use amaru_consensus::{
    consensus::{ChainSyncEvent, ValidateHeaderEvent},
    peer::Peer,
};
use amaru_kernel::{self, Point};
use futures_util::sink::SinkExt;
use gasket::framework::*;
use serde::{Deserialize, Serialize};
use slot_arithmetic::Slot;
use std::fmt;
use tokio::io::{stdin, stdout, AsyncBufReadExt, BufReader, Lines, Stdin, Stdout};
use tokio_util::codec::{FramedWrite, LinesCodec};
use tracing::{error, Span};

#[allow(dead_code)]
#[derive(Debug)]
pub enum InitError {
    IOError(ReaderError),
    DecodingError(String),
    NotInitMessage(ChainSyncMessage),
}

pub async fn read_peer_addresses_from_init(
    reader: &mut impl MessageReader,
) -> Result<Vec<String>, InitError> {
    let input = reader.read().await.map_err(InitError::IOError)?;

    parse_peer_addresses(input)
}

fn parse_peer_addresses(input: Envelope<ChainSyncMessage>) -> Result<Vec<String>, InitError> {
    match input.body {
        ChainSyncMessage::Init { node_ids, .. } => Ok(node_ids),
        msg => Err(InitError::NotInitMessage(msg)),
    }
}

pub trait MessageReader {
    fn read(
        &mut self,
    ) -> impl std::future::Future<Output = Result<Envelope<ChainSyncMessage>, ReaderError>> + Send;
}

fn parse(next_input: &Option<String>) -> Result<Envelope<ChainSyncMessage>, ReaderError> {
    use ReaderError::*;

    match next_input {
        Some(line) => match serde_json::from_str::<Envelope<ChainSyncMessage>>(line) {
            Ok(v) => Ok(v),
            Err(err) => Err(JSONError(err.to_string())),
        },
        None => Err(EndOfFile), // EOF
    }
}

#[derive(Debug, PartialEq)]
pub enum ReaderError {
    IOError(String),
    JSONError(String),
    EndOfFile,
}

pub struct StdinMessageReader {
    reader: Lines<BufReader<Stdin>>,
}

impl Default for StdinMessageReader {
    fn default() -> Self {
        Self::new()
    }
}

impl StdinMessageReader {
    pub fn new() -> Self {
        let reader = BufReader::new(stdin()).lines();
        Self { reader }
    }
}

impl MessageReader for StdinMessageReader {
    async fn read(&mut self) -> Result<Envelope<ChainSyncMessage>, ReaderError> {
        let next_input = Lines::next_line(&mut self.reader).await.map_err(|err| {
            error!("failed to read line from stdin: {}", err);
            ReaderError::IOError(err.to_string())
        })?;

        parse(&next_input)
    }
}

pub struct StringMessageReader {
    lines: Vec<String>,
    index: usize,
}

impl StringMessageReader {}

impl MessageReader for StringMessageReader {
    async fn read(&mut self) -> Result<Envelope<ChainSyncMessage>, ReaderError> {
        if self.index >= self.lines.len() {
            return Err(ReaderError::EndOfFile);
        }

        let msg = parse(&Some(self.lines[self.index].clone()));
        self.index += 1;
        msg
    }
}

pub struct OutputWriter {
    pub writer: FramedWrite<Stdout, LinesCodec>,
}

impl Default for OutputWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputWriter {
    pub fn new() -> Self {
        let writer = FramedWrite::new(stdout(), LinesCodec::new());
        Self { writer }
    }

    pub async fn write(&mut self, messages: Vec<Envelope<ChainSyncMessage>>) {
        for msg in messages {
            let line = serde_json::to_string(&msg).unwrap();
            self.writer.send(line).await.unwrap();
        }
    }
}

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
            ChainSyncMessage::Fwd {
                msg_id,
                slot,
                hash,
                header,
            } => f
                .debug_struct("Fwd")
                .field("msg_id", msg_id)
                .field("slot", slot)
                .field("hash", &hex::encode(&hash.bytes[..hash.bytes.len().min(3)]))
                .field(
                    "header",
                    &hex::encode(&header.bytes.as_slice()[..header.bytes.len().min(4)]),
                )
                .finish(),
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

impl From<&ValidateHeaderEvent> for ChainSyncMessage {
    fn from(event: &ValidateHeaderEvent) -> Self {
        pub use pallas_crypto::hash::Hash;

        match event {
            ValidateHeaderEvent::Validated { point, .. } => {
                let raw_hash: Hash<32> = point.into();
                ChainSyncMessage::Fwd {
                    msg_id: 0,
                    slot: point.slot_or_default(),
                    hash: Bytes {
                        bytes: raw_hash.to_vec(),
                    },
                    header: Bytes { bytes: vec![] }, // FIXME: vec is the full body not the header
                }
            }
            ValidateHeaderEvent::Rollback { .. } => todo!(),
        }
    }
}

pub fn mk_message(
    v: Envelope<ChainSyncMessage>,
    span: Span,
) -> Result<ChainSyncEvent, WorkerError> {
    use ChainSyncMessage::*;

    let peer = Peer { name: v.src };

    match v.body {
        Fwd {
            msg_id: _,
            slot,
            hash,
            header,
        } => Ok(ChainSyncEvent::RollForward {
            peer,
            point: Point::Specific(slot.into(), hash.into()),
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

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use crate::{
        echo::Envelope,
        simulator::{
            bytes::Bytes,
            sync::{parse, read_peer_addresses_from_init, StringMessageReader},
        },
    };
    use amaru_consensus::{consensus::ValidateHeaderEvent, peer::Peer};
    use amaru_kernel::Point;
    use pallas_codec::minicbor;
    use pallas_crypto::hash::Hasher;
    use pallas_primitives::{babbage, Hash};
    use proptest::{
        prelude::{BoxedStrategy, *},
        proptest,
    };
    use slot_arithmetic::Slot;
    use tracing::trace_span;

    use super::{
        ChainSyncMessage::{self, *},
        ReaderError,
    };

    const TEST_HEADER : &str = "828a1b7f18687338385c451b7f3f6a46c620f2bf58201ef19c82fd7039353d2d027e0adcd1588674a1f67673e2ad8898e8ad3a959bba58204346f1024dc69b1c976350d9468792faea3e0f6f8cfcb73a77a9e02ca4ef26f758205ef67af0ce8fd5c7528f2dc014ebbd99ec956a23e4242d8d5425fa488740a98d8258405d3f1a0b3a6516afeba663d58507a47fe8501e4921c2d5363ec3d883678c4c097e06db41d8a560a16055f5115b32379594f99d7db5a32957be678a55b2dd9fda5850ea7a1649b1638fab16565fcdabfd7c89ae6359c422677e24463e7a122237640ffc1f20263b0d1a6e2b9dd2d0b3331df2bd6f4519e96e2d60d8fc88f65c4e403b06da57210ff07d9a8268ad68bb4206001a00013df0582017d93bc9a012ff78060f4080484caea4c31a7381b8cb9208dac53657e4a902938458200f79db338e0ade358ab6d910e14260fc1f2065fc1dbba49c821653816258e56518491b00128fbdd0d9f26d5840d4c013511cbce3c3bf8dd2ef4eac37b3cb5492f1f8ab6f80897c2286c9f7f7e545d045781ad72b6d7196798159fc7241dff87ae9f9dd29f73fe7aad24f49c8008200005901c0129bc460199bc83dc6eaee50d0bbf89f4c43a91c6fd5ff25f181dc31a66420d276821f40d7801eca552684174320483e0ac28eb99f7317695345f6f9d71bb208bbff4ee78d8743d04569833373dfc682cae0bf2f110a90164f8e1e19ed12e36b1fc0884d9e81ae7533b608255fa8a694b1c3da5388cc427b9ade76a3f27892b813789eb814764587eb40c3c5d525fe24d2a3aa3fe3839dbd447dc606689393275b257d8e773878bf40ceed082c3f90fe3e70332322a0d84a57c1f61f7cb2a99920de88e2ab86f3b11b5ae756055718ce817511042c4826fbc6254a42617857323e73abbc7f33303879facc9ccf454820e31d3a271ac4435e6b7ae17c1b78292fedb5ee4c92aa87f797921c71ab626a5e44871761d332c606331b604bb3f069663d7750ad2868d2d9ad80d3bc5a83ce63911dc53395861ae296ec9fe14e2d0c8cbf3402c7d4b2b11107444e7dad3b3f0ce78860baea60ecd02aaab366b6395daa9d1bb1bf80599bfde25c233608c6860a0d7ae51f8487ba3f77e0e2ba5a452c19ae46055b0e6062e6647277b01dca68ba74cc32ced883f26965c551f20ed2a893173bec9eb216efefdffd360a2ed166d82fe3263f826041650d5cc567399f0c9c";

    const TEST_FWD_MSG: &str = r#"{"src":"n2","dest":"n1","body":{"type":"fwd","msg_id":1,"hash":"2487bd4f49c89e59bb3d2166510d3d49017674d3c3b430b95db2e260fedce45e","header":"828a01181ff6582022ff37595005fa65ad731d4fb112de050aa0ca910d9a3110f56f3879d449f88e5820da90997ba81483b3f2b4de751ba3ece6ba6d50f96598eb0940cc7d29452cdcb9825840d350e19abe11a25b28d4d1a846faa8f7792b6dc4a19679780dcdd3d98baf4e9738d82764c59b1c76f80b5ddbff2e145aa26f1652ab83f1f930fc430d1be960305850b444cc452b3fbc01a267ff526ea8a66cb57202303b4b1c22557cad8c1d8afbb47a34e55c5e0bee497f58c812e8b2fed95b40cb1f966ad77445ef573f289910debf5dcb4924cecce47d181f325b4d21040058200a9aa01fbdfe2cff3e49a3c02c2610691966075092f76bd26f5bddf85489ffd3845820499fc5dada1544be26d7cd3b2851fe955b44e560b50901abc71333d5d449eac9182e005840595a9a329b2637b8f2cb501aa793a159acc928a27c01e1ef586508492a68cabeae6ced714421be1f648bb05c7196f38e7aa4a8f616ad46e32c84e67951657a038200005901c0c00b0daf83dfc61a23fa9f6e4db5ad31428a7e98839aba420dbdbdc5ad90b72185cb03b5373b73fcd9c0128b3afcc7d14e5b51f4d5592d55a5d222314b590101472338fcc8b178f0ba10f8683725c5df444b7fc6a5afc3c7fbab82e00e7df2d247c673d066eacc1dc7860c10b134c413d71c5a073a5f3e66a17f0d25dabae2699d7b4a969e129d627cd7839995ddf40a3e6672b6d03936de782f5e0dc31bfafdccc5d5d2e5e9a5b3414bface59a824e3a574250474d633115af821b63232de753fddb638606b93b853144dd75692b02e73b2ef8621eb1bfa307cfda8acaa2c43f8b673c9ed749e472cdf2fced20c063a2507ccb985d2b5a9bc77699f42379fb349e1e3a1ab86d1bd510c6ee89720f860d55c208dd262f49746d8fb2a7817d038262a6b266f98f427fc8b958f11adb8c84cc96444b1f5f9d994a4fd1adae2f2be87d8c1dbc0206ace7871da50cc92476d3fced6a4fc2809c8bb47dff992b06259c21f78cd6e72b49b8fe101065aaf243003af67a11d5aabcebf1059b3c92493f954507573a01bf92047ef68f4e980df914b360307b78b3371138b4c1504e155f704df9fabf1bc87ac47b3d5cd563cee6017380e4df6529face9cf39b36277068e","height":1,"parent":null,"slot":31}}"#;

    fn read_lines_from_vector(lines: Vec<String>) -> StringMessageReader {
        StringMessageReader { lines, index: 0 }
    }

    fn some_forward() -> ChainSyncMessage {
        let header_bytes = hex::decode(TEST_HEADER).unwrap();
        let header: babbage::MintedHeader<'_> = minicbor::decode(&header_bytes).unwrap();
        let header_hash = Hasher::<256>::hash(header.raw_cbor());
        Fwd {
            msg_id: 1,
            slot: Slot::from(1234),
            hash: header_hash.to_vec().into(),
            header: header_bytes.into(),
        }
    }

    #[test]
    fn can_retrieve_forward_from_message() {
        let fwd = some_forward();
        let expected_hash: Hash<32> =
            Hash::from_str("746353a52e80b3ac2d6d51658df7988d4b7baa219f55584a4827bd00bc97617e")
                .unwrap();
        let message = Envelope {
            src: "peer1".to_string(),
            dest: "me".to_string(),
            body: fwd,
        };

        let event = super::mk_message(message, tracing::trace_span!("test")).unwrap();

        match event {
            super::ChainSyncEvent::RollForward {
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

    #[tokio::test]
    async fn can_read_init_message_with_some_peer_addresses() {
        let init_string = r#"{"body":{"node_id":"c0","node_ids":["n1","n2"],"type":"init","msg_id":0},"dest":"c0","src":"c0"}"#;
        let mut input: StringMessageReader = read_lines_from_vector(vec![init_string.to_string()]);

        let envelope = read_peer_addresses_from_init(&mut input).await;

        assert_eq!(envelope.unwrap(), vec!["n1".to_string(), "n2".to_string()]);
    }

    #[tokio::test]
    async fn returns_error_when_reading_addresses_given_message_is_not_init() {
        let mut input: StringMessageReader = read_lines_from_vector(vec![TEST_FWD_MSG.to_string()]);

        let envelope = read_peer_addresses_from_init(&mut input).await;

        assert!(envelope.is_err());
    }

    #[test]
    fn returns_error_when_parsing_message_fails() {
        assert_eq!(
            parse(&Some("foo".to_string())),
            Err(ReaderError::JSONError(
                "expected ident at line 1 column 2".to_string()
            ))
        );
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
                ChainSyncMessage::Fwd {
                    msg_id,
                    slot: Slot::from(slot),
                    hash: hash.to_vec().into(),
                    header: Bytes { bytes: vec![] },
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

    proptest! {
        #[test]
        fn roundtrip_chain_sync_messages(message in arbitrary_message()) {
            let encoded = serde_json::to_string(&message).unwrap();
            let decoded = serde_json::from_str(&encoded).unwrap();
            assert_eq!(message, decoded);
        }

    }

    fn arbitrary_block_validated_event() -> BoxedStrategy<ValidateHeaderEvent> {
        use proptest::prelude::*;
        use ValidateHeaderEvent::*;

        prop_oneof![("[0-9a-z]+", any::<u64>(), any::<[u8; 32]>()).prop_map(
            |(name, slot, hash)| {
                let span = trace_span!("");
                Validated {
                    peer: Peer { name },
                    point: Point::Specific(slot, hash.into()),
                    span,
                }
            }
        )]
        .boxed()
    }

    proptest! {
        #[test]
        fn converts_block_validated_event_to_messages(event in arbitrary_block_validated_event()) {
            let message = ChainSyncMessage::from(&event);
            assert!(matches!(message, ChainSyncMessage::Fwd{..}));
        }
    }
}
