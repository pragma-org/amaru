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

use std::collections::BTreeSet;

use amaru_pure_stage::{
    session::{Agency, ProjectionConfig, SessionSpec, assert_projects},
    session_spec,
    typestate::{RoleTag, labels},
};

use super::{
    BLOCKFETCH_AGENCY_TIMEOUT, BatchDone, Block, ClientDone, Message, NoBlocks, RequestRange, StartBatch,
    initiator::{self, Busy, Done, Idle, Streaming},
    responder,
};
use crate::protocol::{Pull, ToMux, check_want_next};

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

fn blockfetch_initiator() -> ProjectionConfig {
    ProjectionConfig {
        role: Agency::Initiator,
        peer_role: initiator::ToResponder::NAME,
        mux_role: ToMux::NAME,
        local_roles: BTreeSet::from([initiator::ToCollector::NAME]),
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
        local_inputs: labels([initiator::Fetch::LABEL, initiator::Close::LABEL]),
        driven: true,
    }
}

fn blockfetch_responder() -> ProjectionConfig {
    ProjectionConfig {
        role: Agency::Responder,
        peer_role: responder::ToInitiator::NAME,
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
fn protocol_conformance() {
    check_want_next(&initiator::Proto::type_graph(), &blockfetch_initiator()).unwrap();
    check_want_next(&responder::Proto::type_graph(), &blockfetch_responder()).unwrap();
    let spec = session_spec();
    let h_i = assert_projects(&initiator::Proto::type_graph(), &blockfetch_initiator(), &spec);
    let h_r = assert_projects(&responder::Proto::type_graph(), &blockfetch_responder(), &spec);
    h_r.dual().assert_bisimilar(&h_i);
}
