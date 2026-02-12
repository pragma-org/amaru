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
    simulator::{
        Entry, Envelope, bytes::Bytes, data_generation::base_generators::generate_arrival_times,
    },
    sync::ChainSyncMessage,
};
use amaru_consensus::headers_tree::data_generation::{
    Action, GeneratedActions, any_select_chains_from_tree, any_tree_of_headers, transpose,
};
use amaru_kernel::{IsHeader, Peer, Point, to_cbor, utils::tests::run_strategy_with_rng};
use pure_stage::{Instant, simulation::RandStdRng};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::to_value;
use std::{
    collections::BTreeMap,
    fmt::{Debug, Display},
    time::Duration,
};

/// Holds a list of generated entries along with the context used to generate them.
#[derive(Clone, PartialEq, Eq)]
pub struct GeneratedEntries<Msg, GenerationContext> {
    entries: Vec<Entry<Msg>>,
    generation_context: GenerationContext,
}

impl<GenerationContext: Debug> Debug for GeneratedEntries<ChainSyncMessage, GenerationContext> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let lines = self.display_as_lines();
        for line in lines {
            writeln!(f, "{}", line)?;
        }
        self.generation_context.fmt(f)?;
        Ok(())
    }
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
    /// Return the entries as a list of lines, ready to be printed out.
    /// This is used in the Debug implementation but can also be fed to logs
    pub fn display_as_lines(&self) -> Vec<String> {
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
        GeneratedEntry::from(entry.clone()).to_string()
    }
}

impl GeneratedEntries<ChainSyncMessage, GeneratedActions> {
    pub fn as_json(&self) -> serde_json::Value {
        let entries_json: Vec<serde_json::Value> = self
            .entries()
            .iter()
            .map(|entry| to_value(GeneratedEntry::from(entry.clone())).unwrap())
            .collect();

        serde_json::json!({
            "tree": self.generation_context().generated_tree().as_json(),
            "messages": entries_json,
        })
    }

    /// Export the generated entries to a JSON file at the given path.
    pub fn export_to_file(&self, path: &str) {
        use std::{fs::File, io::Write};

        let mut file = File::create(path).unwrap();
        let content = self.as_json().to_string();
        file.write_all(content.as_bytes()).unwrap();
    }
}

/// A single generated entry formatted for display and serialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct GeneratedEntry {
    message_type: String,
    src: String,
    hash: String,
    parent: String,
    slot: u64,
    arrival_time: String,
}

impl Display for GeneratedEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&format!(
            "{message_type:<3} {src} {time:>9} {slot:>5} {hash:>6} (parent {parent_hash:>6})",
            message_type = self.message_type,
            src = self.src,
            time = self.arrival_time,
            slot = self.slot,
            hash = self.hash,
            parent_hash = self.parent,
        ))
    }
}

impl From<Entry<ChainSyncMessage>> for GeneratedEntry {
    fn from(entry: Entry<ChainSyncMessage>) -> Self {
        let message_type = match entry.envelope.body {
            ChainSyncMessage::Fwd { .. } => "FWD",
            ChainSyncMessage::Bck { .. } => "BCK",
            _ => "UNK",
        };
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

        let slot = entry.envelope.body.slot().unwrap_or_default().into();
        let arrival_time = entry.arrival_time.to_string();

        GeneratedEntry {
            message_type: message_type.to_string(),
            src: entry.envelope.src,
            hash: header_hash.to_string().chars().take(6).collect(),
            parent: header_parent_hash.to_string().chars().take(6).collect(),
            slot,
            arrival_time,
        }
    }
}

/// Generates a sequence of chain sync entries based on random actions from peers on a tree of
/// headers generated with a specified depth.
///
/// TODO: since we are generating data with a `proptest` strategy the simulation framework can not
/// for now shrink the list of actions generated here. This means that if a test fails the generated data
/// will not be minimized to find a smaller failing case. The generation is deterministic though based on the
/// RNG passed as a parameter.
pub fn generate_entries(
    chain_length: usize,
    peers: &[Peer],
    start_time: Instant,
    mean_millis: f64,
) -> impl Fn(RandStdRng) -> GeneratedEntries<ChainSyncMessage, GeneratedActions> {
    move |mut rng: RandStdRng| {
        // Generate a tree of headers.
        let generated_tree = run_strategy_with_rng(&mut rng.0, any_tree_of_headers(chain_length));

        // Generate actions corresponding to peers doing roll forwards and roll backs on the tree.
        let generated_actions = run_strategy_with_rng(
            &mut rng.0,
            any_select_chains_from_tree(&generated_tree, peers),
        );

        // Generate arrivale times and make entries for each peer.
        let mut entries_by_peer: BTreeMap<Peer, Vec<Entry<ChainSyncMessage>>> = BTreeMap::new();
        for (peer, actions) in generated_actions.actions_per_peer().iter() {
            // introduce a random start delay for each peer simulate different connection times
            let start_delay = rng.0.random_range(0..(mean_millis as u64 * 10));
            let arrival_times = generate_arrival_times(
                start_time + Duration::from_millis(start_delay),
                mean_millis,
            )(actions.len(), &mut rng.0);
            make_entries_for_peer(&mut entries_by_peer, peer, actions.clone(), arrival_times);
        }

        // Interleave the peers entries to simulate concurrent arrivals.
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

/// Create entries for a given peer based on the actions and arrival times.
fn make_entries_for_peer(
    entries: &mut BTreeMap<Peer, Vec<Entry<ChainSyncMessage>>>,
    peer: &Peer,
    actions: Vec<Action>,
    arrival_times: Vec<Instant>,
) {
    let mut peer_entries = vec![];
    for (msg_id, (action, arrival_time)) in actions
        .into_iter()
        .zip(arrival_times.into_iter())
        .enumerate()
    {
        let message = match &action {
            Action::RollForward { header, .. } => ChainSyncMessage::Fwd {
                msg_id: msg_id as u64,
                slot: header.slot(),
                hash: Bytes {
                    bytes: header.hash().to_vec(),
                },
                header: Bytes {
                    bytes: to_cbor(&header),
                },
            },
            Action::Rollback { rollback_point, .. } => ChainSyncMessage::Bck {
                msg_id: msg_id as u64,
                slot: rollback_point.slot_or_default(),
                hash: Bytes::from(rollback_point.hash().to_vec()),
            },
        };
        peer_entries.push(make_entry(action.peer(), &arrival_time, message));
    }
    entries.insert(peer.clone(), peer_entries);
}

/// Create an entry for a given peer, arrival time and message body.
/// The source name for the envelope is prefixed with "c" to indicate it's a client peer.
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
    use crate::simulator::RunConfig;
    use pure_stage::EPOCH;
    use rand::{SeedableRng, prelude::StdRng};

    /// This test checks that the generated entries have reasonable slot values
    /// compared to their arrival times.
    ///
    /// Additionally this test can be used to generate data for the animation in tests/animations/entries.html.
    #[test]
    fn test_generate_entries() {
        let run_config = RunConfig {
            generated_chain_depth: 15,
            number_of_upstream_peers: 10,
            ..Default::default()
        };
        let start_time = Instant::at_offset(Duration::from_secs(1));
        let deviation_millis = 200.0;

        let rng = StdRng::seed_from_u64(42);
        let upstream_peers = run_config.upstream_peers();
        let generate = generate_entries(
            run_config.generated_chain_depth,
            &upstream_peers,
            start_time,
            deviation_millis,
        );
        let generated_entries = generate(RandStdRng(rng));

        // Uncomment these lines to print the generated entries for debugging
        // for entry in generated_entries.lines() {
        //     println!("{}", entry);
        // }

        // Uncomment this line to generate a new data file for the animation in
        // tests/animations/entries.html.
        // generated_entries.export_to_file("../target/simulation/data.json");

        // The first 20 entries are roll forward messages
        // We expect the slot value to be around 2000ms of the arrival time,
        // given a possible start delay and some jitter around each message arrival.
        for (i, entry) in generated_entries.entries().iter().take(20).enumerate() {
            if let Some(slot) = entry.envelope.body.slot() {
                let arrival_time_ms =
                    entry.arrival_time.saturating_since(*EPOCH).as_millis() as u64;
                let slot_ms = u64::from(slot) * 1000;
                assert!(
                    (arrival_time_ms as f64 - slot_ms as f64).abs() <= 4000.0,
                    "Entry {i} arrival time {arrival_time_ms} ms and slot {slot_ms} ms differ more than 4000 ms",
                )
            }
        }
    }
}
