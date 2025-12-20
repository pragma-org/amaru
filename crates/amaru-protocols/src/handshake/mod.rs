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

use crate::{
    handshake::messages::Message,
    mux::{HandlerMessage, MuxMessage},
    protocol::{NETWORK_SEND_TIMEOUT, Outcome, PROTO_HANDSHAKE, Role, outcome},
};
use amaru_kernel::{
    bytes::NonEmptyBytes,
    protocol_messages::{
        handshake::{HandshakeResult, RefuseReason},
        version_data::VersionData,
    },
};
use pure_stage::{Effects, StageRef, TryInStage};

mod messages;
#[cfg(test)]
mod tests;

pub use messages::VersionTable;

#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub enum HandshakeMessage {
    Registered,
    FromNetwork(NonEmptyBytes),
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub struct Handshake {
    muxer: StageRef<MuxMessage>,
    connection: StageRef<HandshakeResult>,
    state: HandshakeState,
    role: Role,
}

impl Handshake {
    pub fn new(
        muxer: StageRef<MuxMessage>,
        connection: StageRef<HandshakeResult>,
        role: Role,
        version_table: VersionTable<VersionData>,
    ) -> Self {
        Self {
            muxer,
            connection,
            state: HandshakeState::Propose(version_table),
            role,
        }
    }
}

pub async fn stage(
    mut handshake: Handshake,
    msg: HandshakeMessage,
    eff: Effects<HandshakeMessage>,
) -> Handshake {
    let (state, outcome) = match msg {
        HandshakeMessage::Registered => {
            if handshake.role == Role::Initiator {
                handshake
                    .state
                    .initiator_step(Message::Propose(VersionTable::empty()))
                    .or_terminate(&eff, async |err| {
                        tracing::error!(%err, "failed to propose version table");
                    })
                    .await
            } else {
                (handshake.state, outcome())
            }
        }
        HandshakeMessage::FromNetwork(non_empty_bytes) => {
            let msg: Message<VersionData> = minicbor::decode(&non_empty_bytes)
                .or_terminate(&eff, async |err| {
                    tracing::error!(%err, "failed to decode message from network");
                })
                .await;
            match handshake.role {
                Role::Initiator => handshake.state.initiator_step(msg),
                Role::Responder => handshake.state.responder_step(msg),
            }
            .or_terminate(&eff, async |err| {
                tracing::error!(%err, "failed to step handshake");
            })
            .await
        }
    };
    if let Some(msg) = outcome.send {
        let proto_id = PROTO_HANDSHAKE.for_role(handshake.role);
        let msg = NonEmptyBytes::encode(&msg);
        eff.call(&handshake.muxer, NETWORK_SEND_TIMEOUT, move |cr| {
            MuxMessage::Send(proto_id, msg, cr)
        })
        .await;
    }
    if let Some(result) = outcome.result {
        eff.send(&handshake.connection, result).await;
    }
    handshake.state = state;
    handshake
}

#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
enum HandshakeState {
    Propose(VersionTable<VersionData>),
    Confirm(VersionTable<VersionData>),
    Done,
}

impl HandshakeState {
    fn initiator_step(
        self,
        input: Message<VersionData>,
    ) -> anyhow::Result<(Self, Outcome<Message<VersionData>, HandshakeResult>)> {
        Ok(match (self, input) {
            (Self::Propose(version_table), Message::Propose(_)) => (
                Self::Confirm(version_table.clone()),
                outcome().send(Message::Propose(version_table)),
            ),
            (Self::Confirm(_), Message::Accept(version_number, version_data)) => (
                Self::Done,
                outcome().result(HandshakeResult::Accepted(version_number, version_data)),
            ),
            (Self::Confirm(_), Message::Refuse(reason)) => (
                Self::Done,
                outcome().result(HandshakeResult::Refused(reason)),
            ),
            (Self::Confirm(vt), Message::Propose(version_table)) => (
                Self::Done,
                outcome().result(compute_negotiation_result(vt, version_table)),
            ),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }

    fn responder_step(
        self,
        input: Message<VersionData>,
    ) -> anyhow::Result<(Self, Outcome<Message<VersionData>, HandshakeResult>)> {
        Ok(match (self, input) {
            (Self::Propose(vt), Message::Propose(version_table)) => (
                Self::Done,
                outcome().send(compute_negotiation_result(vt, version_table).into()),
            ),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }
}

// FIXME: proper implementation of network spec needed
fn compute_negotiation_result(
    ours: VersionTable<VersionData>,
    theirs: VersionTable<VersionData>,
) -> HandshakeResult {
    let mut their_versions = theirs.values.keys().collect::<Vec<_>>();
    their_versions.sort();
    their_versions.reverse();
    for their_version in their_versions {
        if let Some(our_version) = ours.values.get(their_version) {
            return HandshakeResult::Accepted(*their_version, our_version.clone());
        }
    }
    HandshakeResult::Refused(RefuseReason::VersionMismatch(
        ours.values.keys().copied().collect(),
    ))
}

impl From<HandshakeResult> for Message<VersionData> {
    fn from(result: HandshakeResult) -> Self {
        match result {
            HandshakeResult::Accepted(version_number, version_data) => {
                Message::Accept(version_number, version_data)
            }
            HandshakeResult::Refused(reason) => Message::Refuse(reason),
        }
    }
}

pub fn handler_transform(msg: HandlerMessage) -> HandshakeMessage {
    match msg {
        HandlerMessage::FromNetwork(bytes) => HandshakeMessage::FromNetwork(bytes),
        HandlerMessage::Registered(_) => HandshakeMessage::Registered,
    }
}
