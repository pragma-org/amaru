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

use crate::echo::Envelope;
use crate::simulator::bytes::Bytes;
use crate::simulator::data_generation::base_generators::generate_arrival_times;
use crate::simulator::{Entry, NodeConfig};
use crate::sync::ChainSyncMessage;
use amaru_consensus::consensus::headers_tree::data_generation::{
    Action, GeneratedActions, any_select_chains_from_tree, any_tree_of_headers,
};
use amaru_kernel::{IsHeader, is_header::tests::run_with_rng, peer::Peer, to_cbor};
use amaru_slot_arithmetic::Slot;
use pure_stage::Instant;
use rand::Rng;

/// Generates a sequence of chain sync entries based on random actions from peers on a tree of
/// headers generated with a specified depth.
///
/// FIXME: since we are generating data with a `proptest` strategy the simulation framework can not
/// for now shrink the list of actions generated here. This means that if a test fails the generated data
/// will not be minimized to find a smaller failing case. The generation is deterministic though based on the
/// RNG passed as a parameter.
pub fn generate_entries<R: Rng>(
    node_config: &NodeConfig,
    start_time: Instant,
    mean_millis: f64,
) -> impl Fn(&mut R) -> (Vec<Entry<ChainSyncMessage>>, GeneratedActions) {
    move |rng: &mut R| {
        let generated_tree = run_with_rng(
            rng,
            any_tree_of_headers(node_config.generated_chain_depth as usize),
        );

        let generated_actions = run_with_rng(
            rng,
            any_select_chains_from_tree(
                &generated_tree,
                node_config.number_of_upstream_peers as usize,
            ),
        );

        let arrival_times =
            generate_arrival_times(start_time, mean_millis)(generated_actions.len(), rng);
        let mut messages = Vec::new();

        for (msg_id, (action, arrival_time)) in
            (0_u64..).zip(generated_actions.actions().iter().zip(arrival_times.iter()))
        {
            let message = match &action {
                Action::RollForward { header, .. } => ChainSyncMessage::Fwd {
                    msg_id,
                    slot: Slot::from(header.slot()),
                    hash: Bytes {
                        bytes: header.hash().to_vec(),
                    },
                    header: Bytes {
                        bytes: to_cbor(&header),
                    },
                },
                Action::RollBack { rollback_point, .. } => ChainSyncMessage::Bck {
                    msg_id,
                    slot: rollback_point.slot_or_default(),
                    hash: Bytes::from(rollback_point.hash().to_vec()),
                },
            };
            messages.push(make_entry(action.peer(), arrival_time, message));
        }
        (messages, generated_actions)
    }
}

fn make_entry<T>(peer: &Peer, arrival_time: &Instant, body: T) -> Entry<T> {
    Entry {
        arrival_time: *arrival_time,
        envelope: Envelope {
            src: format!("c{}", peer.name.clone()),
            dest: "n1".to_string(),
            body,
        },
    }
}
