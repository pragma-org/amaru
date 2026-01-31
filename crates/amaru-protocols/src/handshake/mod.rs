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
mod messages;
mod responder;
#[cfg(test)]
mod tests;

use crate::{
    protocol::{ProtoSpec, ProtocolState, Role, RoleT},
    protocol_messages::{
        handshake::{HandshakeResult, RefuseReason},
        version_data::{PEER_SHARING_DISABLED, VersionData},
        version_number::VersionNumber,
        version_table::VersionTable,
    },
};
use amaru_kernel::NetworkMagic;

pub use messages::Message;

pub fn register_deserializers() -> pure_stage::DeserializerGuards {
    vec![
        initiator::register_deserializers(),
        responder::register_deserializers(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[derive(
    Debug, PartialEq, Eq, Clone, Copy, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum State {
    Propose,
    Confirm,
    Done,
}

// Re-export types
pub use initiator::{HandshakeInitiator, initiator};
pub use responder::{HandshakeResponder, responder};

// FIXME: proper implementation of network spec needed
pub fn compute_negotiation_result(
    _role: Role,
    ours: VersionTable<VersionData>,
    theirs: VersionTable<VersionData>,
) -> HandshakeResult {
    use crate::protocol_messages::handshake::RefuseReason;
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

pub fn spec<R: RoleT>() -> ProtoSpec<State, Message<VersionData>, R>
where
    State: ProtocolState<R, WireMsg = Message<VersionData>>,
{
    use State::*;

    let mut spec = ProtoSpec::default();

    let propose = || Message::Propose(VersionTable::empty());
    let accept = || {
        Message::Accept(
            VersionNumber::V14,
            VersionData::new(NetworkMagic::MAINNET, false, PEER_SHARING_DISABLED, false),
        )
    };
    let refuse = || Message::Refuse(RefuseReason::VersionMismatch(vec![VersionNumber::V14]));
    let query_reply = || Message::QueryReply(VersionTable::empty());

    spec.init(Propose, propose(), Confirm);
    spec.sim_open(Confirm, propose(), Done);
    spec.resp(Confirm, accept(), Done);
    spec.resp(Confirm, refuse(), Done);
    spec.resp(Confirm, query_reply(), Done);
    spec
}
