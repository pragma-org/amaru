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
    mux::MuxMessage,
    protocol::{
        Initiator, Miniprotocol, Outcome, PROTO_HANDSHAKE, ProtocolState, Responder, Role, RoleT,
        StageState, miniprotocol, outcome,
    },
};
use amaru_kernel::protocol_messages::{
    handshake::{HandshakeResult, RefuseReason},
    version_data::VersionData,
    version_number::VersionNumber,
    version_table::VersionTable,
};
use pure_stage::{Effects, StageRef, Void};
use std::marker::PhantomData;

mod messages;
#[cfg(test)]
mod tests;

pub fn register_deserializers() -> pure_stage::DeserializerGuards {
    vec![
        pure_stage::register_data_deserializer::<Handshake<Initiator>>().boxed(),
        pure_stage::register_data_deserializer::<Handshake<Responder>>().boxed(),
        pure_stage::register_data_deserializer::<HandshakeResult>().boxed(),
    ]
}

pub fn initiator() -> Miniprotocol<HandshakeState, Handshake<Initiator>, Initiator> {
    miniprotocol(PROTO_HANDSHAKE)
}

pub fn responder() -> Miniprotocol<HandshakeState, Handshake<Responder>, Responder> {
    miniprotocol(PROTO_HANDSHAKE.responder())
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub struct Handshake<R: RoleT> {
    muxer: StageRef<MuxMessage>,
    connection: StageRef<HandshakeResult>,
    our_versions: VersionTable<VersionData>,
    _phantom: PhantomData<R>,
}

impl<R: RoleT> Handshake<R> {
    pub fn new(
        muxer: StageRef<MuxMessage>,
        connection: StageRef<HandshakeResult>,
        version_table: VersionTable<VersionData>,
    ) -> (HandshakeState, Self) {
        (
            HandshakeState::StPropose,
            Self {
                muxer,
                connection,
                our_versions: version_table,
                _phantom: PhantomData,
            },
        )
    }
}

impl<R: RoleT> AsRef<StageRef<MuxMessage>> for Handshake<R> {
    fn as_ref(&self) -> &StageRef<MuxMessage> {
        &self.muxer
    }
}

impl StageState<HandshakeState, Initiator> for Handshake<Initiator> {
    type LocalIn = ();

    async fn local(
        self,
        _proto: &HandshakeState,
        _input: Self::LocalIn,
        _eff: &Effects<crate::protocol::Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(
        Option<<HandshakeState as ProtocolState<Initiator>>::Action>,
        Self,
    )> {
        anyhow::bail!("handshake initiator does not accept local input");
    }

    async fn network(
        self,
        _proto: &HandshakeState,
        input: <HandshakeState as ProtocolState<Initiator>>::Out,
        eff: &Effects<crate::protocol::Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(
        Option<<HandshakeState as ProtocolState<Initiator>>::Action>,
        Self,
    )> {
        Ok(match input {
            InitiatorResult::Propose => {
                tracing::debug!(?self.our_versions, "proposing versions");
                (Some(Proposal(self.our_versions.clone())), self)
            }
            InitiatorResult::Conclusion(handshake_result) => {
                tracing::debug!(?handshake_result, "conclusion");
                eff.send(&self.connection, handshake_result).await;
                (None, self)
            }
            InitiatorResult::SimOpen(version_table) => {
                tracing::debug!(?version_table, "simultaneous open");
                let result = compute_negotiation_result(
                    Role::Initiator,
                    self.our_versions.clone(),
                    version_table,
                );
                eff.send(&self.connection, result).await;
                (None, self)
            }
        })
    }
}

impl StageState<HandshakeState, Responder> for Handshake<Responder> {
    type LocalIn = ();

    async fn local(
        self,
        _proto: &HandshakeState,
        _input: Self::LocalIn,
        _eff: &Effects<crate::protocol::Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(
        Option<<HandshakeState as ProtocolState<Responder>>::Action>,
        Self,
    )> {
        anyhow::bail!("handshake responder does not accept local input");
    }

    async fn network(
        self,
        _proto: &HandshakeState,
        input: <HandshakeState as ProtocolState<Responder>>::Out,
        eff: &Effects<crate::protocol::Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(
        Option<<HandshakeState as ProtocolState<Responder>>::Action>,
        Self,
    )> {
        let result =
            compute_negotiation_result(Role::Responder, self.our_versions.clone(), input.0);
        eff.send(&self.connection, result.clone()).await;
        Ok((Some(result.into()), self))
    }
}

#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub enum HandshakeState {
    StPropose,
    StConfirm,
    StDone,
}

#[derive(Debug)]
pub enum InitiatorResult {
    Propose,
    Conclusion(HandshakeResult),
    SimOpen(VersionTable<VersionData>),
}

#[derive(Debug)]
pub struct Proposal(VersionTable<VersionData>);

impl ProtocolState<Initiator> for HandshakeState {
    type WireMsg = Message<VersionData>;
    type Action = Proposal;
    type Out = InitiatorResult;

    fn init(&self) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)> {
        Ok((outcome().result(InitiatorResult::Propose), Self::StPropose))
    }

    fn network(
        &self,
        input: Self::WireMsg,
    ) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)> {
        Ok(match input {
            Message::Propose(version_table) => {
                // TCP simultaneous open
                (
                    outcome().result(InitiatorResult::SimOpen(version_table)),
                    Self::StDone,
                )
            }
            Message::Accept(version_number, version_data) => (
                outcome().result(InitiatorResult::Conclusion(HandshakeResult::Accepted(
                    version_number,
                    version_data,
                ))),
                Self::StDone,
            ),
            Message::Refuse(refuse_reason) => (
                outcome().result(InitiatorResult::Conclusion(HandshakeResult::Refused(
                    refuse_reason,
                ))),
                Self::StDone,
            ),
            Message::QueryReply(version_table) => (
                outcome().result(InitiatorResult::Conclusion(HandshakeResult::Query(
                    version_table,
                ))),
                Self::StDone,
            ),
        })
    }

    fn local(&self, input: Self::Action) -> anyhow::Result<(Outcome<Self::WireMsg, Void>, Self)> {
        Ok((outcome().send(Message::Propose(input.0)), Self::StConfirm))
    }
}

#[derive(Debug)]
pub enum ResponderAction {
    Accept(VersionNumber, VersionData),
    Refuse(RefuseReason),
    Query(VersionTable<VersionData>),
}

impl ProtocolState<Responder> for HandshakeState {
    type WireMsg = Message<VersionData>;
    type Action = ResponderAction;
    type Out = Proposal;

    fn init(&self) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)> {
        Ok((outcome(), Self::StPropose))
    }

    fn network(
        &self,
        input: Self::WireMsg,
    ) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)> {
        #[expect(clippy::wildcard_enum_match_arm)]
        match input {
            Message::Propose(version_table) => {
                Ok((outcome().result(Proposal(version_table)), Self::StConfirm))
            }
            input => anyhow::bail!("invalid message from initiator: {:?}", input),
        }
    }

    fn local(&self, input: Self::Action) -> anyhow::Result<(Outcome<Self::WireMsg, Void>, Self)> {
        Ok(match input {
            ResponderAction::Accept(version_number, version_data) => (
                outcome().send(Message::Accept(version_number, version_data)),
                Self::StDone,
            ),
            ResponderAction::Refuse(refuse_reason) => {
                (outcome().send(Message::Refuse(refuse_reason)), Self::StDone)
            }
            ResponderAction::Query(version_table) => (
                outcome().send(Message::Propose(version_table)),
                Self::StDone,
            ),
        })
    }
}

// FIXME: proper implementation of network spec needed
fn compute_negotiation_result(
    _role: Role,
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

impl From<HandshakeResult> for ResponderAction {
    fn from(result: HandshakeResult) -> Self {
        match result {
            HandshakeResult::Accepted(version_number, version_data) => {
                ResponderAction::Accept(version_number, version_data)
            }
            HandshakeResult::Refused(reason) => ResponderAction::Refuse(reason),
            HandshakeResult::Query(version_table) => ResponderAction::Query(version_table),
        }
    }
}
