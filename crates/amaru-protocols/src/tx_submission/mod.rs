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

pub mod responder_params;
pub use responder_params::*;

pub mod messages;
pub use messages::*;

pub mod outcome;
pub use outcome::*;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub use tests::*;

use crate::connection::ConnectionMessage;
use crate::mux;
use crate::protocol::{Inputs, PROTO_N2N_TX_SUB, ProtocolState, Role, RoleT};
use amaru_kernel::Tx;
use amaru_ouroboros::TxOrigin;
use amaru_ouroboros_traits::TxId;
use pure_stage::{Effects, StageRef, Void};

pub use initiator::initiator;
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

pub fn spec<R: RoleT>() -> crate::protocol::ProtoSpec<TxSubmissionState, Message, R>
where
    TxSubmissionState: ProtocolState<R, WireMsg = Message>,
{
    let mut spec = crate::protocol::ProtoSpec::default();
    let request_tx_ids_blocking = |start: u16, end: u16| Message::RequestTxIdsBlocking(start, end);
    let request_tx_ids_non_blocking =
        |start: u16, end: u16| Message::RequestTxIdsNonBlocking(start, end);
    let request_txs = |txs: Vec<TxId>| Message::RequestTxs(txs);
    let reply_tx_ids = |tx_ids: Vec<(TxId, u32)>| Message::ReplyTxIds(tx_ids);
    let reply_txs = |txs: Vec<Tx>| Message::ReplyTxs(txs);

    spec.i(
        TxSubmissionState::Init,
        Message::Init,
        TxSubmissionState::Idle,
    );
    spec.i(
        TxSubmissionState::TxIdsBlocking,
        reply_tx_ids(vec![]),
        TxSubmissionState::Idle,
    );
    spec.i(
        TxSubmissionState::TxIdsNonBlocking,
        reply_tx_ids(vec![]),
        TxSubmissionState::Idle,
    );
    spec.i(
        TxSubmissionState::Txs,
        reply_txs(vec![]),
        TxSubmissionState::Idle,
    );
    spec.i(
        TxSubmissionState::Idle,
        Message::Done,
        TxSubmissionState::Done,
    );
    spec.r(
        TxSubmissionState::Idle,
        request_tx_ids_blocking(0, 0),
        TxSubmissionState::TxIdsBlocking,
    );
    spec.r(
        TxSubmissionState::Idle,
        request_tx_ids_non_blocking(0, 0),
        TxSubmissionState::TxIdsNonBlocking,
    );
    spec.r(
        TxSubmissionState::Idle,
        request_txs(vec![]),
        TxSubmissionState::Txs,
    );
    spec
}

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
            Inputs::<Void>::Network,
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

/// The state of the tx submission protocol as a whole.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, serde::Serialize, serde::Deserialize)]
pub enum TxSubmissionState {
    Init,
    Idle,
    Done,
    Txs,
    TxIdsBlocking,
    TxIdsNonBlocking,
}
