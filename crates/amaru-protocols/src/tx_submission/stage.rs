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

use crate::connection::ConnectionMessage;
use crate::mempool_effects::MemoryPool;
use crate::mux;
use crate::mux::{HandlerMessage, MuxMessage};
use crate::protocol::{Erased, PROTO_N2N_TX_SUB, ProtocolId, Role};
use crate::tx_submission::messages::Message;
use crate::tx_submission::{
    Outcome, ResponderParams, TxSubmissionInitiatorState, TxSubmissionMessage,
    TxSubmissionResponderState,
};
use amaru_kernel::bytes::NonEmptyBytes;
use amaru_ouroboros::TxOrigin;
use pure_stage::{Effects, StageRef, TryInStage};
use std::time::Duration;
use tracing;

/// This stage handles the tx submission protocol for the initiator role.
pub async fn initiator_stage(
    mut tx_submission: TxSubmission<TxSubmissionInitiatorState>,
    msg: TxSubmissionMessage,
    eff: Effects<TxSubmissionMessage>,
) -> TxSubmission<TxSubmissionInitiatorState> {
    let (protocol_state, outcome) = match msg {
        TxSubmissionMessage::Registered => {
            tracing::trace!("tx submission protocol registered");
            tx_submission
                .role_state
                .step(
                    &MemoryPool::new(eff.clone()),
                    &tx_submission.protocol_state,
                    Message::Init,
                )
                .await
                .or_terminate(&eff, async |err| {
                    tracing::error!(%err, "failed to initialize");
                })
                .await
        }
        TxSubmissionMessage::FromNetwork(non_empty_bytes) => {
            tracing::trace!("received a message from the network");
            let msg: Message = minicbor::decode(&non_empty_bytes)
                .or_terminate(&eff, async |err| {
                    tracing::error!(%err, "failed to decode message from network");
                })
                .await;
            tx_submission
                .role_state
                .step(
                    &MemoryPool::new(eff.clone()),
                    &tx_submission.protocol_state,
                    msg,
                )
                .await
                .or_terminate(&eff, async |err| {
                    tracing::error!(%err, "failed to step tx submission state machine");
                })
                .await
        }
    };
    process_outcome(
        &tx_submission.muxer,
        &eff,
        outcome,
        PROTO_N2N_TX_SUB.erase(),
    )
    .await;
    tx_submission.protocol_state = protocol_state;
    tx_submission
}

/// This stage handles the tx submission protocol for the responder role.
pub async fn responder_stage(
    mut tx_submission: TxSubmission<TxSubmissionResponderState>,
    msg: TxSubmissionMessage,
    eff: Effects<TxSubmissionMessage>,
) -> TxSubmission<TxSubmissionResponderState> {
    let (protocol_state, outcome) = match msg {
        TxSubmissionMessage::Registered => {
            tracing::trace!("tx submission protocol registered for responder");
            (tx_submission.protocol_state, Outcome::Done)
        }
        TxSubmissionMessage::FromNetwork(non_empty_bytes) => {
            tracing::trace!("received a message from the network");
            let msg: Message = minicbor::decode(&non_empty_bytes)
                .or_terminate(&eff, async |err| {
                    tracing::error!(%err, "failed to decode message from network");
                })
                .await;
            tx_submission
                .role_state
                .step(
                    &MemoryPool::new(eff.clone()),
                    &tx_submission.protocol_state,
                    msg,
                )
                .await
                .or_terminate(&eff, async |err| {
                    tracing::error!(%err, "failed to step tx submission state machine");
                })
                .await
        }
    };

    process_outcome(
        &tx_submission.muxer,
        &eff,
        outcome,
        PROTO_N2N_TX_SUB.responder().erase(),
    )
    .await;
    tx_submission.protocol_state = protocol_state;
    tx_submission
}

/// Execute the outcome of a protocol step and request the next message if needed.
async fn process_outcome(
    mux: &StageRef<MuxMessage>,
    eff: &Effects<TxSubmissionMessage>,
    outcome: Outcome,
    protocol: ProtocolId<Erased>,
) {
    match outcome {
        Outcome::Send(msg) => {
            tracing::trace!(msg_type = ?msg.message_type(), "sending message to network");
            eff.call(mux, NETWORK_SEND_TIMEOUT, move |cr| {
                MuxMessage::Send(protocol, to_bytes(msg), cr)
            })
            .await;
            tracing::trace!("require the next message from the tx submission protocol");
            eff.send(mux, MuxMessage::WantNext(protocol)).await;
        }
        Outcome::Error(error) => {
            tracing::warn!(%error, "tx submission protocol error, terminating connection");
            eff.terminate::<()>().await;
        }
        Outcome::Done => {
            tracing::trace!("done with tx submission protocol, terminating connection");
            eff.terminate::<()>().await;
        }
    }
}

/// Register the tx submission protocol stage for the given role.
/// This function is used in the Connection stage once the handshake is complete.
pub async fn register_tx_submission(
    role: Role,
    mux: StageRef<MuxMessage>,
    eff: &Effects<ConnectionMessage>,
    origin: TxOrigin,
) -> StageRef<TxSubmissionMessage> {
    let tx_submission = match role {
        Role::Initiator => {
            let tx_submission = eff.stage("tx_submission", initiator_stage).await;
            eff.wire_up(
                tx_submission,
                TxSubmission::new(mux.clone(), TxSubmissionInitiatorState::new()),
            )
            .await
        }
        Role::Responder => {
            let tx_submission = eff.stage("tx_submission", responder_stage).await;
            eff.wire_up(
                tx_submission,
                TxSubmission::new(
                    mux.clone(),
                    TxSubmissionResponderState::new(ResponderParams::new(2, 3), origin),
                ),
            )
            .await
        }
    };
    let tx_submission_bytes = eff
        .stage(
            "tx_submission_bytes",
            async |tx_submission, msg: HandlerMessage, eff| {
                let msg = match msg {
                    HandlerMessage::FromNetwork(bytes) => TxSubmissionMessage::FromNetwork(bytes),
                    HandlerMessage::Registered(_) => TxSubmissionMessage::Registered,
                };
                eff.send(&tx_submission, msg).await;
                tx_submission
            },
        )
        .await;

    let tx_submission_bytes = eff
        .wire_up(tx_submission_bytes, tx_submission.clone())
        .await;
    eff.send(
        &mux,
        MuxMessage::Register {
            protocol: PROTO_N2N_TX_SUB.erase(),
            frame: mux::Frame::OneCborItem,
            handler: tx_submission_bytes,
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

/// Shared state for both initiator and responder roles.
/// The role-specific state is stored in `role_state`.
#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub struct TxSubmission<R> {
    muxer: StageRef<MuxMessage>,
    protocol_state: TxSubmissionState,
    role_state: R,
}

impl<R> TxSubmission<R> {
    pub fn new(muxer: StageRef<MuxMessage>, role_state: R) -> Self {
        Self {
            muxer,
            protocol_state: TxSubmissionState::Init,
            role_state,
        }
    }
}

#[expect(clippy::expect_used)]
fn to_bytes(msg: Message) -> NonEmptyBytes {
    NonEmptyBytes::from_slice(&minicbor::to_vec(&msg).expect("failed to encode message"))
        .expect("guaranteed by message format")
}

// FIXME find right value
const NETWORK_SEND_TIMEOUT: Duration = Duration::from_secs(1);
