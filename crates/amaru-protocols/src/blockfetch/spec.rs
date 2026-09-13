// Copyright 2026 PRAGMA
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

//! Network-spec BlockFetch machine (Tables 3.7 / 3.8) as a [`SessionSpec`].
//!
//! State keys and message labels are type names (`Idle`, `RequestRange`, …),
//! not dummy payload values.

use std::collections::BTreeSet;

use amaru_pure_stage::{
    session::{Agency, ProjectionConfig, SessionSpec, project},
    session_spec,
    typestate::{RoleTag, TypeGraph, labels},
};

use super::{
    BLOCKFETCH_AGENCY_TIMEOUT, BatchDone, Block, ClientDone, Message, NoBlocks, RequestRange, StartBatch,
    initiator::{Busy, Close, Done, Fetch, Idle, Streaming, ToCollector, ToResponder},
    responder::ToInitiator,
};
use crate::protocol::{Pull, ToMux};

pub(crate) fn session_spec() -> SessionSpec {
    session_spec! {
        Message;
        [*] --> Idle
        Idle --> Busy: RequestRange
        Idle --> Done: ClientDone
        Busy --> Idle: NoBlocks
        Busy --> Streaming: StartBatch
        Streaming --> Streaming: Block
        Streaming --> Idle: BatchDone
        note left of Idle: Initiator
        note left of Busy: Responder timeout BLOCKFETCH_AGENCY_TIMEOUT
        note left of Streaming: Responder timeout BLOCKFETCH_AGENCY_TIMEOUT
    }
}

pub(crate) fn blockfetch_initiator() -> ProjectionConfig {
    ProjectionConfig {
        role: Agency::Initiator,
        peer_role: ToResponder::NAME,
        mux_role: ToMux::NAME,
        local_roles: BTreeSet::from([ToCollector::NAME]),
        wire_inputs: labels([StartBatch::LABEL, NoBlocks::LABEL, Block::LABEL, BatchDone::LABEL]),
        wire_payload: labels([
            RequestRange::LABEL,
            ClientDone::LABEL,
            StartBatch::LABEL,
            NoBlocks::LABEL,
            Block::LABEL,
            BatchDone::LABEL,
        ]),
        plumbing_inputs: labels([Pull::LABEL]),
        local_inputs: labels([Fetch::LABEL, Close::LABEL]),
        driven: true,
    }
}

pub(crate) fn blockfetch_responder() -> ProjectionConfig {
    ProjectionConfig {
        role: Agency::Responder,
        peer_role: ToInitiator::NAME,
        mux_role: ToMux::NAME,
        local_roles: BTreeSet::new(),
        wire_inputs: labels([RequestRange::LABEL, ClientDone::LABEL]),
        wire_payload: labels([
            RequestRange::LABEL,
            ClientDone::LABEL,
            StartBatch::LABEL,
            NoBlocks::LABEL,
            Block::LABEL,
            BatchDone::LABEL,
        ]),
        plumbing_inputs: labels([Pull::LABEL]),
        local_inputs: BTreeSet::new(),
        driven: false,
    }
}

#[test]
fn collapsed_responder_dual_equals_collapsed_initiator() {
    let h_i = project(&super::initiator::Proto::type_graph(), &blockfetch_initiator()).unwrap();
    let h_r = project(&super::responder::Proto::type_graph(), &blockfetch_responder()).unwrap();
    h_r.dual().assert_bisimilar(&h_i);
}
