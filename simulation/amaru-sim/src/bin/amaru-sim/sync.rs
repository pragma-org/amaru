use amaru_kernel::Point;
use amaru_ouroboros::protocol::{peer::Peer, PullEvent};
use gasket::framework::*;
use maelstrom::protocol::Message;
use serde::{Deserialize, Serialize};
use std::io;
use tracing::{error, trace_span, Span};

use crate::bytes::Bytes;

pub type DownstreamPort = gasket::messaging::OutputPort<PullEvent>;

pub enum WorkUnit {
    Send(PullEvent),
}

#[derive(Stage)]
#[stage(name = "pull", unit = "WorkUnit", worker = "Worker")]
pub struct Stage {
    pub downstream: DownstreamPort,
}

impl Stage {
    pub(crate) fn new(_tip: &Point) -> Self {
        Self {
            downstream: DownstreamPort::default(),
        }
    }
}

pub struct Worker {}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<Stage> for Worker {
    async fn bootstrap(_stage: &Stage) -> Result<Self, WorkerError> {
        let worker = Self {};

        Ok(worker)
    }

    async fn schedule(
        &mut self,
        _stage: &mut Stage,
    ) -> Result<WorkSchedule<WorkUnit>, WorkerError> {
        // read one line of stdin which should be a JSON-formatted message from
        // some peer to our peer
        let mut input = String::new();
        let span = trace_span!("pull-worker");

        io::stdin()
            .read_line(&mut input)
            .map_err(|_| WorkerError::Recv)?;

        match serde_json::from_str::<Message>(input.as_str()) {
            Ok(v) => {
                let msg = mk_message(v, span)?;
                Ok(WorkSchedule::Unit(WorkUnit::Send(msg)))
            }
            Err(err) => {
                error!("failed to deserialize input {}", err);
                Err(WorkerError::Recv)
            }
        }
    }

    async fn execute(&mut self, unit: &WorkUnit, stage: &mut Stage) -> Result<(), WorkerError> {
        match unit {
            WorkUnit::Send(event) => stage.downstream.send(event.clone().into()).await.or_panic(),
        }
    }
}

#[derive(Debug, PartialEq, Deserialize, Serialize)]
pub struct Fwd {
    slot: u64,
    hash: Bytes,
    header: Bytes,
}

#[derive(Debug, PartialEq, Deserialize, Serialize)]
pub struct Bck {
    slot: u64,
    hash: Bytes,
}

fn mk_message(v: Message, span: Span) -> Result<PullEvent, WorkerError> {
    let peer = Peer { name: v.src };

    match v.body.typ.as_str() {
        "forward" => {
            let Fwd { slot, hash, header } =
                serde_json::from_value::<Fwd>(v.body.raw()).or_panic()?;
            Ok(PullEvent::RollForward(
                peer,
                Point::Specific(slot, hash.into()),
                header.into(),
                span,
            ))
        }
        "rollback" => {
            let Bck { slot, hash } = serde_json::from_value::<Bck>(v.body.raw()).or_panic()?;
            Ok(PullEvent::Rollback(
                peer,
                Point::Specific(slot, hash.into()),
                span,
            ))
        }
        _ => Err(WorkerError::Recv),
    }
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use super::{Bck, Fwd};
    use amaru_consensus::consensus::header::point_hash;
    use maelstrom::protocol::{Message, MessageBody};
    use pallas_codec::minicbor;
    use pallas_crypto::hash::Hasher;
    use pallas_primitives::{babbage, Hash};
    use proptest::prelude::*;
    use serde_json::Value;

    const TEST_HEADER : &str = "828a1b7f18687338385c451b7f3f6a46c620f2bf58201ef19c82fd7039353d2d027e0adcd1588674a1f67673e2ad8898e8ad3a959bba58204346f1024dc69b1c976350d9468792faea3e0f6f8cfcb73a77a9e02ca4ef26f758205ef67af0ce8fd5c7528f2dc014ebbd99ec956a23e4242d8d5425fa488740a98d8258405d3f1a0b3a6516afeba663d58507a47fe8501e4921c2d5363ec3d883678c4c097e06db41d8a560a16055f5115b32379594f99d7db5a32957be678a55b2dd9fda5850ea7a1649b1638fab16565fcdabfd7c89ae6359c422677e24463e7a122237640ffc1f20263b0d1a6e2b9dd2d0b3331df2bd6f4519e96e2d60d8fc88f65c4e403b06da57210ff07d9a8268ad68bb4206001a00013df0582017d93bc9a012ff78060f4080484caea4c31a7381b8cb9208dac53657e4a902938458200f79db338e0ade358ab6d910e14260fc1f2065fc1dbba49c821653816258e56518491b00128fbdd0d9f26d5840d4c013511cbce3c3bf8dd2ef4eac37b3cb5492f1f8ab6f80897c2286c9f7f7e545d045781ad72b6d7196798159fc7241dff87ae9f9dd29f73fe7aad24f49c8008200005901c0129bc460199bc83dc6eaee50d0bbf89f4c43a91c6fd5ff25f181dc31a66420d276821f40d7801eca552684174320483e0ac28eb99f7317695345f6f9d71bb208bbff4ee78d8743d04569833373dfc682cae0bf2f110a90164f8e1e19ed12e36b1fc0884d9e81ae7533b608255fa8a694b1c3da5388cc427b9ade76a3f27892b813789eb814764587eb40c3c5d525fe24d2a3aa3fe3839dbd447dc606689393275b257d8e773878bf40ceed082c3f90fe3e70332322a0d84a57c1f61f7cb2a99920de88e2ab86f3b11b5ae756055718ce817511042c4826fbc6254a42617857323e73abbc7f33303879facc9ccf454820e31d3a271ac4435e6b7ae17c1b78292fedb5ee4c92aa87f797921c71ab626a5e44871761d332c606331b604bb3f069663d7750ad2868d2d9ad80d3bc5a83ce63911dc53395861ae296ec9fe14e2d0c8cbf3402c7d4b2b11107444e7dad3b3f0ce78860baea60ecd02aaab366b6395daa9d1bb1bf80599bfde25c233608c6860a0d7ae51f8487ba3f77e0e2ba5a452c19ae46055b0e6062e6647277b01dca68ba74cc32ced883f26965c551f20ed2a893173bec9eb216efefdffd360a2ed166d82fe3263f826041650d5cc567399f0c9c";

    fn some_forward() -> Fwd {
        let header_bytes = hex::decode(TEST_HEADER).unwrap();
        let header: babbage::MintedHeader<'_> = minicbor::decode(&header_bytes).unwrap();
        let header_hash = Hasher::<256>::hash(header.raw_cbor());
        Fwd {
            slot: 1234,
            hash: header_hash.to_vec().into(),
            header: header_bytes.into(),
        }
    }

    #[test]
    fn can_deserialize_serialized_forward() {
        let fwd = some_forward();
        let input = serde_json::to_string(&fwd).unwrap();

        assert_eq!(fwd, serde_json::from_str(&input).unwrap());
    }

    proptest! {
        #[test]
        fn can_deserialize_serialized_rollback(slot in any::<u64>(), hash in any::<[u8 ; 32]>()) {
            let rollback = Bck { slot, hash: hash.to_vec().into() };
            let input = serde_json::to_string(&rollback).unwrap();

            println!("input: {}", input);

            assert_eq!(rollback, serde_json::from_str(&input).unwrap());
        }
    }

    #[test]
    fn can_retrieve_forward_from_message() {
        let fwd = some_forward();
        let body = match serde_json::to_value(fwd) {
            Ok(v) => match v {
                Value::Object(m) => m,
                _ => panic!("response object has invalid serde_json::Value kind"),
            },
            Err(e) => panic!("response object is invalid, can't convert: {}", e),
        };

        let message = Message {
            src: "peer1".to_string(),
            dest: "me".to_string(),
            body: MessageBody::from_extra(body).with_type("forward"),
        };

        let event = super::mk_message(message, tracing::trace_span!("test")).unwrap();

        println!("event {:?}", event);
        match event {
            super::PullEvent::RollForward(peer, point, header, _) => {
                assert_eq!(peer.name, "peer1");
                assert_eq!(point.slot_or_default(), 1234);
                assert_eq!(
                    point_hash(&point),
                    Hash::from_str(
                        "746353a52e80b3ac2d6d51658df7988d4b7baa219f55584a4827bd00bc97617e"
                    )
                    .unwrap()
                );
                assert_eq!(header, hex::decode(TEST_HEADER).unwrap());
            }
            _ => panic!("expected RollForward event"),
        }
    }
}
