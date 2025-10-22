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
    Action, GeneratedActions, any_select_chains_from_tree, any_tree_of_headers, transpose,
};
use amaru_kernel::{IsHeader, is_header::tests::run_with_rng, peer::Peer, Point, to_cbor};
use amaru_slot_arithmetic::Slot;
use pure_stage::Instant;
use rand::Rng;
use std::collections::BTreeMap;

/// Holds a list of generated entries along with the context used to generate them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedEntries<Msg, GenerationContext> {
    entries: Vec<Entry<Msg>>,
    generation_context: GenerationContext,
}

impl<Msg, GenerationContext> GeneratedEntries<Msg, GenerationContext> {
    pub fn new(entries: Vec<Entry<Msg>>, generation_context: GenerationContext) -> Self {
        Self {
            entries,
            generation_context,
        }
    }

    pub fn entries(&self) -> &Vec<Entry<Msg>> {
        &self.entries
    }

    pub fn generation_context(&self) -> &GenerationContext {
        &self.generation_context
    }
}

impl<GenerationContext> GeneratedEntries<ChainSyncMessage, GenerationContext> {
    /// Return the entries as a list of lines, ready to be printed out
    pub fn lines(&self) -> Vec<String> {
        let entries = self.entries();
        let mut result = vec![];
        result.push("ALL ENTRIES".to_string());
        for entry in entries.iter() {
            result.push(Self::display_entry(entry))
        }

        result.push("BY PEER".to_string());
        let mut by_peer: BTreeMap<String, Vec<Entry<ChainSyncMessage>>> = BTreeMap::new();
        for entry in entries.iter() {
            by_peer
                .entry(entry.envelope.src.clone())
                .or_default()
                .push(entry.clone());
        }

        for (peer, entries) in by_peer {
            result.push(format!("\nEntries from peer {}", peer));
            for entry in entries.iter() {
                result.push(Self::display_entry(entry))
            }
        }

        result
    }

    /// Display a single entry as a formatted string
    fn display_entry(entry: &Entry<ChainSyncMessage>) -> String {
        let header_hash = entry
            .envelope
            .body
            .header_hash()
            .unwrap_or(Point::Origin.hash());
        let header_parent_hash = entry
            .envelope
            .body
            .header_parent_hash()
            .unwrap_or(Point::Origin.hash());
        let slot = entry.envelope.body.slot().unwrap_or_default();
        let message_type = match entry.envelope.body {
            ChainSyncMessage::Fwd { .. } => "FWD",
            ChainSyncMessage::Bck { .. } => "BCK",
            _ => "UNK",
        };
        format!(
            "{message_type:<3} {src} {time:>9} {slot:>5} {hash:>6} (parent {parent_hash:>6})",
            src = entry.envelope.src,
            time = entry.arrival_time.to_string(),
            slot = slot.to_string(),
            hash = &header_hash.to_string()[..6],
            parent_hash = &header_parent_hash.to_string()[..6]
        )
    }
}

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
) -> impl Fn(&mut R) -> GeneratedEntries<ChainSyncMessage, GeneratedActions> {
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

        let mut entries_by_peer: BTreeMap<Peer, Vec<Entry<ChainSyncMessage>>> = BTreeMap::new();

        for (peer, actions) in generated_actions.actions_per_peer().iter() {
            let arrival_times = generate_arrival_times(start_time, mean_millis)(actions.len(), rng);
            make_entries_for_peer(&mut entries_by_peer, peer, actions.clone(), arrival_times);
        }

        let entries = transpose(entries_by_peer.values())
            .into_iter()
            .flatten()
            .cloned()
            .collect();

        GeneratedEntries {
            entries,
            generation_context: generated_actions,
        }
    }
}

fn make_entries_for_peer(
    entries: &mut BTreeMap<Peer, Vec<Entry<ChainSyncMessage>>>,
    peer: &Peer,
    actions: Vec<Action>,
    arrival_times: Vec<Instant>,
) {
    let mut peer_entries = vec![];
    for (action, arrival_time) in actions.into_iter().zip(arrival_times.into_iter()) {
        let message = match &action {
            Action::RollForward { header, .. } => ChainSyncMessage::Fwd {
                msg_id: 0, // Placeholder, should be set properly
                slot: Slot::from(header.slot()),
                hash: Bytes {
                    bytes: header.hash().to_vec(),
                },
                header: Bytes {
                    bytes: to_cbor(&header),
                },
            },
            Action::RollBack { rollback_point, .. } => ChainSyncMessage::Bck {
                msg_id: 0, // Placeholder, should be set properly
                slot: rollback_point.slot_or_default(),
                hash: Bytes::from(rollback_point.hash().to_vec()),
            },
        };
        peer_entries.push(make_entry(action.peer(), &arrival_time, message));
    }
    entries.insert(peer.clone(), peer_entries);
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

#[cfg(test)]
mod tests {
    use super::*;
    use pure_stage::EPOCH;
    use rand::SeedableRng;
    use rand::prelude::StdRng;

    #[test]
    fn test_generate_entries() {
        let node_config = NodeConfig {
            number_of_upstream_peers: 2,
            number_of_downstream_peers: 1,
            generated_chain_depth: 10,
        };
        let start_time = Instant::at_offset(std::time::Duration::from_secs(1));
        let deviation_millis = 200.0;

        let mut rng = StdRng::seed_from_u64(42);
        let generate = generate_entries(&node_config, start_time, deviation_millis);
        let generated_entries = generate(&mut rng);

        // The first 20 entries are roll forward messages from two peers
        // We expect the slot value to be within 250 ms of the arrival time.
        for (i, entry) in generated_entries.entries().iter().take(20).enumerate() {
            if let Some(slot) = entry.envelope.body.slot() {
                let arrival_time_ms =
                    entry.arrival_time.saturating_since(*EPOCH).as_millis() as u64;
                let slot_ms = u64::from(slot) * 1000;
                assert!(
                    (arrival_time_ms as f64 - slot_ms as f64).abs() <= 400.0,
                    "Entry {i} arrival time {arrival_time_ms} ms and slot {slot_ms} ms differ more than 400 ms",
                )
            }
        }

        // Uncomment these lines to print the generated entries for debugging
        // for entry in generated_entries.lines() {
        //     println!("{}", entry);
        // }
    }
}
