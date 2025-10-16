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
use amaru_consensus::IsHeader;
use amaru_consensus::consensus::headers_tree::data_generation::{Action, Ratio, any_select_chains};
use amaru_kernel::peer::Peer;
use amaru_kernel::to_cbor;
use amaru_ouroboros_traits::tests::run_with_rng;
use amaru_slot_arithmetic::Slot;
use pure_stage::Instant;
use rand::Rng;

pub fn generate_entries<R: Rng>(
    node_config: &NodeConfig,
    start_time: Instant,
    mean_millis: f64,
) -> impl Fn(&mut R) -> Vec<Entry<ChainSyncMessage>> + use<'_, R> {
    move |rng: &mut R| {
        let actions = run_with_rng(
            rng,
            any_select_chains(
                node_config.number_of_upstream_peers as usize,
                node_config.generated_chain_depth as usize,
                Ratio(1, 2),
            ),
        );

        let arrival_times = generate_arrival_times(start_time, mean_millis)(actions.len(), rng);
        let mut messages = Vec::new();

        for (msg_id, (action, arrival_time)) in
            (0_u64..).zip(actions.into_iter().zip(arrival_times.iter()))
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
        messages
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
