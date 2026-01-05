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
    handshake::{HandshakeState, messages::Message},
    mux::MuxMessage,
    protocol::{
        Initiator, Inputs, Miniprotocol, Outcome, PROTO_HANDSHAKE, ProtocolState, StageState,
        miniprotocol, outcome,
    },
};
use amaru_kernel::protocol_messages::{
    handshake::HandshakeResult, version_data::VersionData, version_table::VersionTable,
};
use pure_stage::{DeserializerGuards, Effects, StageRef, Void};

pub fn register_deserializers() -> DeserializerGuards {
    vec![pure_stage::register_data_deserializer::<HandshakeInitiator>().boxed()]
}

pub fn initiator() -> Miniprotocol<HandshakeState, HandshakeInitiator, Initiator> {
    miniprotocol(PROTO_HANDSHAKE)
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub struct HandshakeInitiator {
    muxer: StageRef<MuxMessage>,
    connection: StageRef<HandshakeResult>,
    our_versions: VersionTable<VersionData>,
}

impl HandshakeInitiator {
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
            },
        )
    }
}

impl AsRef<StageRef<MuxMessage>> for HandshakeInitiator {
    fn as_ref(&self) -> &StageRef<MuxMessage> {
        &self.muxer
    }
}

impl StageState<HandshakeState, Initiator> for HandshakeInitiator {
    type LocalIn = Void;

    async fn local(
        self,
        _proto: &HandshakeState,
        input: Self::LocalIn,
        _eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<InitiatorAction>, Self)> {
        match input {}
    }

    async fn network(
        self,
        _proto: &HandshakeState,
        input: InitiatorResult,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<InitiatorAction>, Self)> {
        Ok(match input {
            InitiatorResult::Propose => {
                tracing::debug!(?self.our_versions, "proposing versions");
                (
                    Some(InitiatorAction::Propose(self.our_versions.clone())),
                    self,
                )
            }
            InitiatorResult::Conclusion(handshake_result) => {
                tracing::debug!(?handshake_result, "conclusion");
                eff.send(&self.connection, handshake_result).await;
                (None, self)
            }
            InitiatorResult::SimOpen(version_table) => {
                tracing::debug!(?version_table, "simultaneous open");
                let result = crate::handshake::compute_negotiation_result(
                    crate::protocol::Role::Initiator,
                    self.our_versions.clone(),
                    version_table,
                );
                eff.send(&self.connection, result).await;
                (None, self)
            }
        })
    }
}

impl ProtocolState<Initiator> for HandshakeState {
    type WireMsg = Message<VersionData>;
    type Action = InitiatorAction;
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
        Ok(match input {
            InitiatorAction::Propose(version_table) => (
                outcome().send(Message::Propose(version_table)),
                Self::StConfirm,
            ),
        })
    }
}

#[derive(Debug)]
pub enum InitiatorResult {
    Propose,
    Conclusion(HandshakeResult),
    SimOpen(VersionTable<VersionData>),
}

#[derive(Debug)]
pub enum InitiatorAction {
    Propose(VersionTable<VersionData>),
}

#[cfg(test)]
#[expect(clippy::wildcard_enum_match_arm)]
pub mod tests {
    use crate::handshake::HandshakeState;
    use crate::handshake::initiator::InitiatorAction;
    use crate::handshake::messages::Message;
    use crate::protocol::{Initiator, Role};

    #[test]
    fn test_initiator_protocol() {
        crate::handshake::spec::<Initiator>().check(
            HandshakeState::StPropose,
            Role::Initiator,
            |msg| match msg {
                Message::Propose(version_table) => {
                    Some(InitiatorAction::Propose(version_table.clone()))
                }
                _ => None,
            },
            |msg| msg.clone(),
        );
    }
}
