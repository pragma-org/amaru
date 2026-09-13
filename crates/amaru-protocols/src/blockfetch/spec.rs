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
    typestate::{RoleTag, StateName, TypeGraph},
    typestate_graph,
};

use super::{
    BatchDone, Block, ClientDone, Message, NoBlocks, RequestRange, StartBatch,
    initiator::{BLOCKFETCH_AGENCY_TIMEOUT, Busy, Close, Done, Fetch, Idle, Streaming, ToCollector, ToResponder},
    responder::ToInitiator,
};
use crate::{
    protocol::{ProjectionConfig, Pull, Role, SessionSpec, StateId, ToMux, project},
    session_input_names, session_labels, session_spec,
};

pub(crate) fn session_spec() -> SessionSpec<StateName, &'static str> {
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

impl ProjectionConfig<&'static str> {
    pub(crate) fn blockfetch_initiator() -> Self {
        Self {
            role: Role::Initiator,
            peer_role: ToResponder::NAME,
            mux_role: ToMux::NAME,
            local_roles: BTreeSet::from([ToCollector::NAME]),
            wire_inputs: session_labels!(Message; StartBatch, NoBlocks, Block, BatchDone),
            wire_payload: session_labels!(
                Message;
                RequestRange, ClientDone, StartBatch, NoBlocks, Block, BatchDone
            ),
            plumbing_inputs: session_input_names!(Pull),
            local_inputs: session_input_names!(Fetch, Close),
            driven: true,
        }
    }

    pub(crate) fn blockfetch_responder() -> Self {
        Self {
            role: Role::Responder,
            peer_role: ToInitiator::NAME,
            mux_role: ToMux::NAME,
            local_roles: BTreeSet::new(),
            wire_inputs: session_labels!(Message; RequestRange, ClientDone),
            wire_payload: session_labels!(
                Message;
                RequestRange, ClientDone, StartBatch, NoBlocks, Block, BatchDone
            ),
            plumbing_inputs: session_input_names!(Pull),
            local_inputs: BTreeSet::new(),
            driven: false,
        }
    }
}

pub(crate) fn assert_wire_inputs_cover_receives(graph: &TypeGraph, cfg: &ProjectionConfig<&'static str>) {
    for (state, inputs) in &graph.receives {
        for input in inputs.keys() {
            if cfg.plumbing_inputs.contains(input) || cfg.local_inputs.contains(input) {
                continue;
            }
            assert!(cfg.wire_inputs.contains_key(input), "wire receive arm {input} at {state} is not in wire_inputs");
        }
    }
    let table: BTreeSet<&str> =
        session_spec().transitions.values().flat_map(|per| per.transitions.keys().copied()).collect();
    for label in cfg.wire_inputs.values().chain(cfg.wire_payload.values()) {
        assert!(table.contains(label), "wire map label {label} is not in session_spec()");
    }
}

pub(crate) fn map_i(state: &StateId) -> StateId {
    match state {
        StateId::Named("Idle" | "Busy" | "Streaming" | "Done") => state.clone(),
        StateId::Named(other) => panic!("unexpected named state {other}"),
        StateId::Synthetic { parent, path } => panic!("unexpected synthetic {parent}#{path:?}"),
    }
}

pub(crate) fn map_r(state: &StateId) -> StateId {
    match state {
        StateId::Named("Idle" | "Done") => state.clone(),
        StateId::Named(other) => panic!("unexpected named state {other}"),
        StateId::Synthetic { parent: "Idle", path } if path.as_slice() == ["RequestRange"] => StateId::Named("Busy"),
        StateId::Synthetic { parent: "Idle", path } if path.as_slice() == ["RequestRange", "StartBatch"] => {
            StateId::Named("Streaming")
        }
        StateId::Synthetic { parent, path } => panic!("unexpected synthetic {parent}#{path:?}"),
    }
}

pub(crate) fn initiator_type_graph() -> TypeGraph {
    typestate_graph! {
        proto: super::initiator::Proto,
        receiving: { Idle, Busy, Streaming },
        empty: { Done },
    }
}

pub(crate) fn responder_type_graph() -> TypeGraph {
    use super::responder::{Done, Idle, Proto};
    typestate_graph! {
        proto: Proto,
        receiving: { Idle },
        empty: { Done },
    }
}

#[test]
fn collapsed_responder_dual_equals_collapsed_initiator() {
    let h_i = project(&initiator_type_graph(), &ProjectionConfig::blockfetch_initiator()).unwrap();
    let h_r = project(&responder_type_graph(), &ProjectionConfig::blockfetch_responder()).unwrap();
    h_r.collapse(map_r).dual().assert_bisimilar(&h_i.collapse(map_i));
}
