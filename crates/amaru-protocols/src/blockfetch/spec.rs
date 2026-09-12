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
//! State keys are the initiator constructor names (`Idle`, `Busy`, `Streaming`,
//! `Done`). Dummy [`Message`] values here are the same values used in
//! [`ProjectionConfig`] wire maps.

use std::collections::{BTreeMap, BTreeSet};

use amaru_kernel::NetworkPoint;
use amaru_pure_stage::{
    typestate::{PayloadName, RoleTag, State, StateName, TypeGraph},
    typestate_graph,
};

use super::{
    BatchDone, Block, ClientDone, Message, NoBlocks, RequestRange, StartBatch,
    initiator::{BLOCKFETCH_AGENCY_TIMEOUT, Busy, Done, Idle, Streaming, ToCollector, ToResponder},
    responder::ToInitiator,
};
use crate::protocol::{ProjectionConfig, Role, SessionSpec, StateId, ToMux, project};

fn dummy_request_range() -> Message {
    RequestRange { from: NetworkPoint::Origin, through: NetworkPoint::Origin }.into()
}

fn dummy_client_done() -> Message {
    ClientDone.into()
}

fn dummy_start_batch() -> Message {
    StartBatch.into()
}

fn dummy_no_blocks() -> Message {
    NoBlocks.into()
}

fn dummy_block() -> Message {
    Block { body: Vec::new() }.into()
}

fn dummy_batch_done() -> Message {
    BatchDone.into()
}

/// One listing feeds the exhaustive `Message` match and `dummy_messages()`.
/// A new variant cannot compile the match without entering the alphabet.
macro_rules! blockfetch_dummies {
    ($($var:ident => $dummy:expr),+ $(,)?) => {
        fn dummy_payload(msg: &Message) -> (PayloadName, Message) {
            match msg {
                $(Message::$var(_) => (stringify!($var), $dummy),)+
            }
        }

        fn dummy_of_same_variant(msg: &Message) -> Message {
            dummy_payload(msg).1
        }

        pub(crate) fn dummy_messages() -> BTreeMap<PayloadName, Message> {
            [$((stringify!($var), $dummy),)+].into_iter().collect()
        }
    };
}

blockfetch_dummies! {
    RequestRange => dummy_request_range(),
    ClientDone => dummy_client_done(),
    StartBatch => dummy_start_batch(),
    NoBlocks => dummy_no_blocks(),
    Block => dummy_block(),
    BatchDone => dummy_batch_done(),
}

pub(crate) fn session_spec() -> SessionSpec<StateName, Message> {
    let mut spec = SessionSpec::default();
    spec.init(Idle::NAME, dummy_request_range(), Busy::NAME);
    spec.init(Idle::NAME, dummy_client_done(), Done::NAME);
    spec.resp(Busy::NAME, dummy_no_blocks(), Idle::NAME);
    spec.resp(Busy::NAME, dummy_start_batch(), Streaming::NAME);
    spec.resp(Streaming::NAME, dummy_block(), Streaming::NAME);
    spec.resp(Streaming::NAME, dummy_batch_done(), Idle::NAME);
    spec.set_timeout(Busy::NAME, BLOCKFETCH_AGENCY_TIMEOUT);
    spec.set_timeout(Streaming::NAME, BLOCKFETCH_AGENCY_TIMEOUT);
    spec
}

impl ProjectionConfig<Message> {
    pub(crate) fn blockfetch_initiator() -> Self {
        Self {
            role: Role::Initiator,
            peer_role: ToResponder::NAME,
            mux_role: ToMux::NAME,
            local_roles: BTreeSet::from([ToCollector::NAME]),
            wire_inputs: BTreeMap::from([
                ("StartBatch", dummy_start_batch()),
                ("NoBlocks", dummy_no_blocks()),
                ("Block", dummy_block()),
                ("BatchDone", dummy_batch_done()),
            ]),
            wire_payload: dummy_messages(),
            plumbing_inputs: BTreeSet::from(["Pull"]),
            local_inputs: BTreeSet::from(["Fetch", "Close"]),
            driven: true,
        }
    }

    pub(crate) fn blockfetch_responder() -> Self {
        Self {
            role: Role::Responder,
            peer_role: ToInitiator::NAME,
            mux_role: ToMux::NAME,
            local_roles: BTreeSet::new(),
            wire_inputs: BTreeMap::from([("RequestRange", dummy_request_range()), ("ClientDone", dummy_client_done())]),
            wire_payload: dummy_messages(),
            plumbing_inputs: BTreeSet::from(["Pull"]),
            local_inputs: BTreeSet::new(),
            driven: false,
        }
    }
}

pub(crate) fn assert_message_alphabet_covered(spec: &SessionSpec<StateName, Message>, unused: &[Message]) {
    let table: BTreeSet<Message> = spec.transitions.values().flat_map(|per| per.transitions.keys().cloned()).collect();
    let unused: BTreeSet<Message> = unused.iter().cloned().collect();
    for msg in table.iter().chain(&unused) {
        assert_eq!(*msg, dummy_of_same_variant(msg), "spec/unused dummy must be the canonical payload for {msg:?}");
    }
    for dummy in dummy_messages().into_values() {
        assert!(
            table.contains(&dummy) || unused.contains(&dummy),
            "Message {dummy:?} is neither in the spec table nor listed unused"
        );
    }
}

pub(crate) fn assert_wire_inputs_cover_receives(graph: &TypeGraph, cfg: &ProjectionConfig<Message>) {
    for (state, inputs) in &graph.receives {
        for input in inputs.keys() {
            if cfg.plumbing_inputs.contains(input) || cfg.local_inputs.contains(input) {
                continue;
            }
            assert!(cfg.wire_inputs.contains_key(input), "wire receive arm {input} at {state} is not in wire_inputs");
        }
    }
    for msg in cfg.wire_inputs.values().chain(cfg.wire_payload.values()) {
        assert_eq!(*msg, dummy_of_same_variant(msg), "wire map dummy must equal session_spec() dummy for {msg:?}");
    }
    assert_eq!(cfg.wire_payload, dummy_messages());
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
