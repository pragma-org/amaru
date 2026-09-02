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

use std::sync::Arc;

use amaru_kernel::{EraHistory, Peer};
use amaru_ouroboros::{MempoolMsg, TxOrigin};
use amaru_pure_stage::{Effects, StageRef};
pub use initiator::{InitiatorLocalIn, initiator};
pub use responder::{ResponderLocalIn, ResponderResult, TxSubmissionMsg, responder};
#[cfg(test)]
pub use tests::*;

use crate::{
    connection::ConnectionMessage,
    mux,
    protocol::{Inputs, PROTO_N2N_TX_SUB, ProtocolState, Role, RoleT},
};

pub fn register_deserializers() -> amaru_pure_stage::DeserializerGuards {
    vec![initiator::register_deserializers(), responder::register_deserializers()].into_iter().flatten().collect()
}

pub fn spec<R: RoleT>() -> crate::protocol::ProtoSpec<State, Message, R>
where
    State: ProtocolState<R, WireMsg = Message>,
{
    use Message::*;
    use State::*;
    let mut spec = crate::protocol::ProtoSpec::default();
    let request_tx_ids_blocking = || RequestTxIdsBlocking(0, 0);
    let request_tx_ids_non_blocking = || RequestTxIdsNonBlocking(0, 0);
    let request_txs = || RequestTxs(vec![]);
    let reply_tx_ids = || ReplyTxIds(vec![]);
    let reply_txs = || ReplyTxs(vec![]);

    spec.init(State::Init, Message::Init, Idle);
    spec.init(TxIdsBlocking, reply_tx_ids(), Idle);
    spec.init(TxIdsNonBlocking, reply_tx_ids(), Idle);
    spec.init(Txs, reply_txs(), Idle);
    if R::ROLE == Some(crate::protocol::Role::Initiator) {
        spec.init(TxIdsBlocking, Message::Done, State::Done);
    } else {
        spec.init(TxIdsBlocking, Message::Done, State::Init);
    }
    spec.resp(Idle, request_tx_ids_blocking(), TxIdsBlocking);
    spec.resp(Idle, request_tx_ids_non_blocking(), TxIdsNonBlocking);
    spec.resp(Idle, request_txs(), Txs);
    spec
}

#[expect(clippy::too_many_arguments)]
pub async fn register_tx_submission(
    role: Role,
    peer: Peer,
    muxer: StageRef<mux::MuxMessage>,
    eff: &Effects<ConnectionMessage>,
    origin: TxOrigin,
    mempool_stage: StageRef<MempoolMsg>,
    params: ResponderParams,
    era_history: Arc<EraHistory>,
    tombstone: ConnectionMessage,
) -> Option<StageRef<initiator::InitiatorLocalIn>> {
    let (handler, close) = if role == Role::Initiator {
        let (state, stage) =
            initiator::TxSubmissionInitiator::new(peer, muxer.clone(), mempool_stage.clone(), era_history);
        let tx_submission = eff.stage("tx_submission", initiator::initiator()).await;
        let tx_submission = eff.supervise(tx_submission, tombstone);
        let tx_submission = eff.wire_up(tx_submission, (state, stage)).await;
        (
            tx_submission.contramap(Inputs::<initiator::InitiatorLocalIn>::Network),
            Some(tx_submission.contramap(Inputs::<initiator::InitiatorLocalIn>::Local)),
        )
    } else {
        let (state, stage) =
            responder::TxSubmissionResponder::new(peer, muxer.clone(), params, origin, mempool_stage, era_history);
        let tx_submission = eff.stage("tx_submission-responder", responder::responder()).await;
        let tx_submission = eff.supervise(tx_submission, tombstone);
        let tx_submission = eff.wire_up(tx_submission, (state, stage)).await;
        (tx_submission.contramap(Inputs::<ResponderLocalIn>::Network), None)
    };

    eff.send(
        &muxer,
        mux::MuxMessage::Register {
            protocol: PROTO_N2N_TX_SUB.for_role(role).erase(),
            frame: mux::Frame::OneCborItem,
            handler,
            max_buffer: 2_500_000,
        },
    )
    .await;

    close
}

/// The state of the tx submission protocol as a whole.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum State {
    Init,
    Idle,
    Done,
    Txs,
    TxIdsBlocking,
    TxIdsNonBlocking,
}
