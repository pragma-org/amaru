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
mod responder;

pub mod initiator_state;
pub use initiator_state::*;

pub mod responder_state;
pub use responder_state::*;

pub mod responder_params;
pub use responder_params::*;

pub mod messages;
pub use messages::*;

pub mod outcome;
pub use outcome::*;

pub mod stage;
pub use stage::*;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub use tests::*;

use crate::connection::ConnectionMessage;
use crate::mux;
use crate::protocol::{Inputs, PROTO_N2N_TX_SUB, Role};
use amaru_ouroboros::TxOrigin;
use pure_stage::{Effects, StageRef, Void};

pub use initiator::{InitiatorMessage, initiator};
pub use responder::{ResponderResult, responder};

pub fn register_deserializers() -> pure_stage::DeserializerGuards {
    vec![
        initiator::register_deserializers(),
        responder::register_deserializers(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

// TODO: Implement spec() function once Message implements Ord
// Message currently cannot implement Ord because it contains Vec and Tx which don't implement Ord
// We may need to use a different approach for protocol validation, or create a simplified
// message type for spec validation
/*
pub fn spec<R: RoleT>() -> crate::protocol::ProtoSpec<TxSubmissionState, Message, R>
where
    TxSubmissionState: ProtocolState<R, WireMsg = Message>,
    Message: Ord,
{
    let mut spec = crate::protocol::ProtoSpec::default();

    // Initiator transitions
    // Init -> send Init -> Idle
    spec.i(TxSubmissionState::Init, Message::Init, TxSubmissionState::Idle);
    // Idle -> receive RequestTxIds -> Idle (stays in Idle, sends ReplyTxIds)
    spec.i(
        TxSubmissionState::Idle,
        Message::RequestTxIds(0, 0, crate::tx_submission::Blocking::Yes),
        TxSubmissionState::Idle,
    );
    // Idle -> receive RequestTxs -> Idle (stays in Idle, sends ReplyTxs)
    spec.i(TxSubmissionState::Idle, Message::RequestTxs(vec![]), TxSubmissionState::Idle);

    // Responder transitions
    // Init -> receive Init -> Init (then sends RequestTxIds, transitions to TxIdsBlocking)
    spec.r(TxSubmissionState::Init, Message::Init, TxSubmissionState::TxIdsBlocking);
    // TxIdsBlocking/TxIdsNonBlocking -> receive ReplyTxIds -> Txs
    spec.r(TxSubmissionState::TxIdsBlocking, Message::ReplyTxIds(vec![]), TxSubmissionState::Txs);
    spec.r(
        TxSubmissionState::TxIdsNonBlocking,
        Message::ReplyTxIds(vec![]),
        TxSubmissionState::Txs,
    );
    // Txs -> receive ReplyTxs -> TxIdsBlocking or TxIdsNonBlocking
    spec.r(TxSubmissionState::Txs, Message::ReplyTxs(vec![]), TxSubmissionState::TxIdsBlocking);
    spec.r(
        TxSubmissionState::Txs,
        Message::ReplyTxs(vec![]),
        TxSubmissionState::TxIdsNonBlocking,
    );

    spec
}
*/

pub async fn register_tx_submission(
    role: Role,
    muxer: StageRef<crate::mux::MuxMessage>,
    eff: &Effects<ConnectionMessage>,
    origin: TxOrigin,
) -> StageRef<crate::mux::HandlerMessage> {
    let tx_submission = if role == Role::Initiator {
        let (state, stage) = initiator::TxSubmissionInitiator::new(muxer.clone());
        let tx_submission = eff
            .wire_up(
                eff.stage("tx_submission", initiator::initiator()).await,
                (state, stage),
            )
            .await;
        eff.contramap(
            &tx_submission,
            "tx_submission_handler",
            Inputs::<InitiatorMessage>::Network,
        )
        .await
    } else {
        let (state, stage) = responder::TxSubmissionResponder::new(
            muxer.clone(),
            ResponderParams::new(2, 3),
            origin,
        );
        let tx_submission = eff
            .wire_up(
                eff.stage("tx_submission", responder::responder()).await,
                (state, stage),
            )
            .await;
        eff.contramap(
            &tx_submission,
            "tx_submission_handler",
            Inputs::<Void>::Network,
        )
        .await
    };

    eff.send(
        &muxer,
        crate::mux::MuxMessage::Register {
            protocol: PROTO_N2N_TX_SUB.for_role(role).erase(),
            frame: mux::Frame::OneCborItem,
            handler: tx_submission.clone(),
            max_buffer: 5760,
        },
    )
    .await;

    tx_submission
}
