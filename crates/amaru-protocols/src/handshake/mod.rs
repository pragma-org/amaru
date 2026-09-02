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

use amaru_kernel::NetworkMagic;
pub use messages::Message;

use crate::{
    protocol::{ProtoSpec, ProtocolState, RoleT},
    protocol_messages::{
        handshake::{HandshakeResult, RefuseReason},
        version_data::{PEER_SHARING_DISABLED, VersionData},
        version_number::VersionNumber,
        version_table::VersionTable,
    },
};

pub fn register_deserializers() -> amaru_pure_stage::DeserializerGuards {
    vec![initiator::register_deserializers(), responder::register_deserializers()].into_iter().flatten().collect()
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum State {
    Propose,
    Confirm,
    Done,
}

// Re-export types
pub use initiator::{HandshakeInitiator, initiator};
pub use responder::{HandshakeResponder, responder};

/// Pick the greatest common version and combine that version's data.
///
/// On `query`, the result carries **our** table (the responder sends `MsgQueryReply` with its own
/// offer; the initiator only concludes locally). Simultaneous open uses the same combination.
pub(crate) fn compute_negotiation_result(
    ours: &VersionTable<VersionData>,
    theirs: &VersionTable<VersionData>,
) -> HandshakeResult {
    let Some(version) = ours.values.keys().rev().find(|v| theirs.values.contains_key(v)).copied() else {
        return HandshakeResult::Refused(RefuseReason::VersionMismatch(ours.values.keys().copied().collect()));
    };
    let our_data = &ours.values[&version];
    let their_data = &theirs.values[&version];
    match our_data.combine(their_data) {
        Err(msg) => HandshakeResult::Refused(RefuseReason::Refused(version, msg.to_string())),
        Ok(agreed) if agreed.query() => HandshakeResult::Query(ours.clone()),
        Ok(agreed) => HandshakeResult::Accepted(version, agreed),
    }
}

/// Initiator check of `MsgAcceptVersion`: the record must be the combination of our offer
/// for that version with the record on the wire. `query` is not a session (`MsgQueryReply`).
pub(crate) fn verify_accept(
    ours: &VersionTable<VersionData>,
    version: VersionNumber,
    accepted: VersionData,
) -> HandshakeResult {
    let Some(our_data) = ours.values.get(&version) else {
        return HandshakeResult::Refused(RefuseReason::Refused(version, "version not offered".to_string()));
    };
    match our_data.combine(&accepted) {
        Err(msg) => HandshakeResult::Refused(RefuseReason::Refused(version, msg.to_string())),
        Ok(agreed) if agreed.query() => {
            HandshakeResult::Refused(RefuseReason::Refused(version, "query is not a session".to_string()))
        }
        Ok(agreed) if agreed != accepted => HandshakeResult::Refused(RefuseReason::Refused(
            version,
            "version data is not the agreed record".to_string(),
        )),
        Ok(agreed) => HandshakeResult::Accepted(version, agreed),
    }
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

#[cfg(test)]
#[expect(clippy::wildcard_enum_match_arm)]
mod negotiation_tests {
    use amaru_kernel::{NetworkMagic, cbor};

    use super::*;
    use crate::protocol_messages::version_data::PEER_SHARING_ENABLED;

    fn data(magic: NetworkMagic, initiator_only: bool, sharing: bool, query: bool) -> VersionData {
        VersionData::new(
            magic,
            initiator_only,
            if sharing { PEER_SHARING_ENABLED } else { PEER_SHARING_DISABLED },
            query,
        )
    }

    fn table(entries: &[(u64, VersionData)]) -> VersionTable<VersionData> {
        VersionTable { values: entries.iter().map(|(v, d)| (VersionNumber::new(*v), d.clone())).collect() }
    }

    #[test]
    fn picks_greatest_common_version() {
        let magic = NetworkMagic::PREPROD;
        let ours = table(&[(11, data(magic, false, true, false)), (14, data(magic, false, true, false))]);
        let theirs = table(&[(12, data(magic, false, false, false)), (14, data(magic, false, true, false))]);
        assert_eq!(
            compute_negotiation_result(&ours, &theirs),
            HandshakeResult::Accepted(VersionNumber::V14, data(magic, false, true, false))
        );
    }

    #[test]
    fn version_mismatch_lists_our_versions() {
        let magic = NetworkMagic::PREPROD;
        let ours = table(&[(14, data(magic, false, false, false))]);
        let theirs = table(&[(11, data(magic, false, false, false))]);
        assert_eq!(
            compute_negotiation_result(&ours, &theirs),
            HandshakeResult::Refused(RefuseReason::VersionMismatch(vec![VersionNumber::V14]))
        );
    }

    #[test]
    fn refuses_network_magic_mismatch() {
        let ours = table(&[(14, data(NetworkMagic::PREPROD, false, false, false))]);
        let theirs = table(&[(14, data(NetworkMagic::MAINNET, false, false, false))]);
        assert_eq!(
            compute_negotiation_result(&ours, &theirs),
            HandshakeResult::Refused(RefuseReason::Refused(VersionNumber::V14, "network magic mismatch".to_string()))
        );
    }

    #[test]
    fn initiator_only_is_or() {
        let magic = NetworkMagic::PREPROD;
        let ours = table(&[(14, data(magic, true, false, false))]);
        let theirs = table(&[(14, data(magic, false, false, false))]);
        match compute_negotiation_result(&ours, &theirs) {
            HandshakeResult::Accepted(VersionNumber::V14, agreed) => {
                assert!(agreed.initiator_only_diffusion_mode());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn peer_sharing_is_and() {
        let magic = NetworkMagic::PREPROD;
        let ours = table(&[(14, data(magic, false, true, false))]);
        let theirs = table(&[(14, data(magic, false, false, false))]);
        match compute_negotiation_result(&ours, &theirs) {
            HandshakeResult::Accepted(VersionNumber::V14, agreed) => {
                assert!(!agreed.is_advertisable());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn query_returns_our_table() {
        let magic = NetworkMagic::PREPROD;
        let ours = table(&[(14, data(magic, false, true, true))]);
        let theirs = table(&[(14, data(magic, false, true, false))]);
        assert_eq!(compute_negotiation_result(&ours, &theirs), HandshakeResult::Query(ours));
    }

    #[test]
    fn verify_accept_requires_agreed_record() {
        let magic = NetworkMagic::PREPROD;
        let ours = table(&[(14, data(magic, true, true, false))]);
        let honest = data(magic, true, true, false);
        assert_eq!(
            verify_accept(&ours, VersionNumber::V14, honest.clone()),
            HandshakeResult::Accepted(VersionNumber::V14, honest)
        );
        let lying = data(magic, false, true, false);
        assert_eq!(
            verify_accept(&ours, VersionNumber::V14, lying),
            HandshakeResult::Refused(RefuseReason::Refused(
                VersionNumber::V14,
                "version data is not the agreed record".to_string()
            ))
        );
    }

    #[test]
    fn verify_accept_rejects_query_as_session() {
        let magic = NetworkMagic::PREPROD;
        let ours = table(&[(14, data(magic, false, false, true))]);
        assert_eq!(
            verify_accept(&ours, VersionNumber::V14, data(magic, false, false, true)),
            HandshakeResult::Refused(RefuseReason::Refused(VersionNumber::V14, "query is not a session".to_string()))
        );
    }

    #[test]
    fn outbound_offer_is_duplex() {
        let table = VersionTable::v11_and_above(NetworkMagic::PREPROD, false, true);
        for data in table.values.values() {
            assert!(!data.initiator_only_diffusion_mode());
        }
    }

    fn decode_hex(hex: &str) -> Message<VersionData> {
        let bytes = hex::decode(hex).expect(hex);
        cbor::decode(&bytes).unwrap_or_else(|e| panic!("{hex}: {e}"))
    }

    /// Blueprint `handshake/test-data` vectors (hex of one CBOR message).
    #[test]
    fn blueprint_handshake_vectors_decode() {
        assert!(matches!(decode_hex("8200a0"), Message::Propose(t) if t.values.is_empty()));
        assert_eq!(
            decode_hex("820283020d617b"),
            Message::Refuse(RefuseReason::Refused(VersionNumber::new(13), "{".to_string()))
        );
        match decode_hex("8200a10e8400f401f4") {
            Message::Propose(t) => {
                let d = t.values.get(&VersionNumber::V14).expect("v14");
                assert_eq!(d.network_magic(), NetworkMagic::new(0));
                assert!(!d.initiator_only_diffusion_mode());
                assert!(d.is_advertisable());
                assert!(!d.query());
            }
            other => panic!("{other:?}"),
        }
        match decode_hex("8200a20d8401f501f40e8402f501f4") {
            Message::Propose(t) => {
                assert_eq!(t.values.len(), 2);
                assert_eq!(t.values[&VersionNumber::new(13)].network_magic(), NetworkMagic::new(1));
                assert_eq!(t.values[&VersionNumber::V14].network_magic(), NetworkMagic::new(2));
            }
            other => panic!("{other:?}"),
        }
        match decode_hex("83010e8401f401f4") {
            Message::Accept(VersionNumber::V14, d) => {
                assert_eq!(d.network_magic(), NetworkMagic::new(1));
                assert!(!d.initiator_only_diffusion_mode());
                assert!(d.is_advertisable());
                assert!(!d.query());
            }
            other => panic!("{other:?}"),
        }
    }
}
