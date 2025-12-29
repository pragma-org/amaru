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
        Inputs, Miniprotocol, Outcome, PROTO_HANDSHAKE, ProtocolState, Responder, StageState,
        miniprotocol, outcome,
    },
};
use amaru_kernel::protocol_messages::{
    handshake::{HandshakeResult, RefuseReason},
    version_data::VersionData,
    version_number::VersionNumber,
    version_table::VersionTable,
};
use pure_stage::{DeserializerGuards, Effects, StageRef, Void};

pub fn register_deserializers() -> DeserializerGuards {
    vec![pure_stage::register_data_deserializer::<HandshakeResponder>().boxed()]
}

pub fn responder() -> Miniprotocol<HandshakeState, HandshakeResponder, Responder> {
    miniprotocol(PROTO_HANDSHAKE.responder())
}

#[derive(Debug, PartialEq, Clone, serde::Serialize, serde::Deserialize)]
pub struct HandshakeResponder {
    muxer: StageRef<MuxMessage>,
    connection: StageRef<HandshakeResult>,
    our_versions: VersionTable<VersionData>,
}

impl HandshakeResponder {
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

impl AsRef<StageRef<MuxMessage>> for HandshakeResponder {
    fn as_ref(&self) -> &StageRef<MuxMessage> {
        &self.muxer
    }
}

impl StageState<HandshakeState, Responder> for HandshakeResponder {
    type LocalIn = Void;

    async fn local(
        self,
        _proto: &HandshakeState,
        input: Self::LocalIn,
        _eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<ResponderAction>, Self)> {
        match input {}
    }

    async fn network(
        self,
        _proto: &HandshakeState,
        input: Proposal,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<ResponderAction>, Self)> {
        let result = crate::handshake::compute_negotiation_result(
            crate::protocol::Role::Responder,
            self.our_versions.clone(),
            input.0,
        );
        eff.send(&self.connection, result.clone()).await;
        Ok((Some(result.into()), self))
    }
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

#[derive(Debug)]
pub enum ResponderAction {
    Accept(VersionNumber, VersionData),
    Refuse(RefuseReason),
    Query(VersionTable<VersionData>),
}

#[derive(Debug)]
pub struct Proposal(VersionTable<VersionData>);

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

#[cfg(test)]
pub mod tests {
    use crate::handshake::HandshakeState;
    use crate::handshake::messages::Message;
    use crate::handshake::responder::ResponderAction;
    use crate::protocol::{Responder, Role};

    #[test]
    fn test_responder_protocol() {
        crate::handshake::spec::<Responder>().check(
            HandshakeState::StPropose,
            Role::Responder,
            |msg| match msg {
                Message::Propose(version_table) => {
                    Some(ResponderAction::Query(version_table.clone()))
                }
                _ => None,
            },
            |msg| msg.clone(),
        );
    }
}
