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

mod initiator;
pub(crate) mod messages;
mod responder;
#[cfg(test)]
mod spec;

use std::time::Duration;

use amaru_pure_stage::DeserializerGuards;
#[cfg(test)]
pub(crate) use initiator::BLOCKFETCH_MAX_BLOCK_WIRE_BYTES;
pub use initiator::{
    BLOCKFETCH_PIPELINE_N, BlockFetchMessage, Blocks, blockfetch_handler_mailbox, blockfetch_pipeline_max_buffer,
    register_blockfetch_initiator,
};
pub use messages::{BatchDone, Block, ClientDone, Message, NoBlocks, RequestRange, StartBatch};
pub use responder::register_blockfetch_responder;

/// Receive timeout while the responder has agency (`StBusy` / `StStreaming`).
///
/// From the Cardano Blueprint networking notes: `StIdle` has no receive timeout;
/// `StBusy` and `StStreaming` wait at most 60 seconds.
pub const BLOCKFETCH_AGENCY_TIMEOUT: Duration = Duration::from_secs(60);

pub fn register_deserializers() -> DeserializerGuards {
    vec![initiator::register_deserializers(), responder::register_deserializers()].into_iter().flatten().collect()
}

#[cfg(test)]
mod encode_once {
    use amaru_kernel::{NetworkPoint, NonEmptyBytes};
    use amaru_pure_stage::{StageRef, typestate::IntoRoleCall};

    use super::{Block, Message, RequestRange};
    use crate::{
        mux::MuxMessage,
        protocol::{MuxClient, NETWORK_SEND_TIMEOUT, PROTO_N2N_BLOCK_FETCH, egress_admission_deadline},
    };

    fn client() -> MuxClient {
        MuxClient::new(StageRef::named_for_tests("mux"), PROTO_N2N_BLOCK_FETCH.erase())
    }

    fn assert_send(timeout: std::time::Duration, mail: MuxMessage, wire: &Message) {
        let MuxMessage::Send(_, bytes, _) = mail else {
            panic!("call must be a mux send");
        };
        let encoded = NonEmptyBytes::encode(wire);
        assert_eq!(bytes.as_ref(), encoded.as_ref());
        assert_eq!(timeout, egress_admission_deadline(bytes.len().get()));
        assert_ne!(timeout, NETWORK_SEND_TIMEOUT);
    }

    #[test]
    fn initiator_deadline_is_the_encoded_range() {
        let msg = RequestRange { from: NetworkPoint::Origin, through: NetworkPoint::Origin };
        let wire = Message::from(msg.clone());
        let (timeout, build) = IntoRoleCall::<super::initiator::ToResponder, _>::into_call(client(), msg);
        assert_send(timeout, build(StageRef::named_for_tests("reply")), &wire);
    }

    #[test]
    fn responder_deadline_is_the_encoded_block() {
        // The encoded CBOR item takes one millisecond more wire time than the
        // 112 B body, so a deadline taken from the raw body does not match.
        let body = vec![0xab; 112];
        let msg = Block { body: body.clone() };
        let wire = Message::from(msg.clone());
        let (timeout, build) = IntoRoleCall::<super::responder::ToInitiator, _>::into_call(client(), msg);
        assert_send(timeout, build(StageRef::named_for_tests("reply")), &wire);
        let encoded = NonEmptyBytes::encode(&wire);
        assert_ne!(encoded.len().get(), body.len());
        assert_ne!(egress_admission_deadline(encoded.len().get()), egress_admission_deadline(body.len()));
    }
}
