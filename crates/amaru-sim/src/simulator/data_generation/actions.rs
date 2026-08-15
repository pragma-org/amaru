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

//! Generate peer roll-forward / rollback walks over a tree of headers.

use std::{
    collections::BTreeMap,
    fmt::{Debug, Display, Formatter},
};

use amaru_kernel::{Header, IsHeader, Peer};
use amaru_node::tests::Action;
use proptest::prelude::Strategy;
use rand::{Rng, SeedableRng, prelude::SmallRng};
use serde::{Deserialize, Serialize};
use serde_json::to_value;

use super::{GeneratedTree, Tree, any_tree_of_headers, shrink::Shrinkable};

/// Generate a random list of Actions for a given peer on a given tree of headers.
pub fn random_walk<R: Rng>(
    rng: &mut R,
    parent_header: Option<Header>,
    tree: &Tree<Header>,
    peer: &Peer,
    result: &mut BTreeMap<Peer, Vec<Action>>,
) {
    if !result.contains_key(peer) {
        result.insert(peer.clone(), vec![]);
    }

    if let Some(actions) = result.get_mut(peer) {
        actions.push(Action::RollForward { peer: peer.clone(), header: tree.value.clone() })
    }

    // Process the children in a random order based on a the rng
    let mut children: Vec<_> = tree.children.clone().into_iter().collect();
    children.sort_by_key(|_c| rng.random_bool(0.5));

    // Start a new random walk for each child
    for child in children.iter() {
        random_walk(rng, Some(tree.value.clone()), child, peer, result);
    }

    // Come back to the parent node to explore another tree branch
    if let Some(parent) = parent_header
        && let Some(actions) = result.get_mut(peer)
    {
        let rollback = Action::Rollback { peer: peer.clone(), rollback_point: parent.point().to_network_point() };
        if actions.last().map(|h| h.hash()) != Some(rollback.hash()) {
            actions.push(rollback)
        }
    }
}

/// Generate random walks for a fixed number of peers on a given tree of headers.
///
/// The returned list of actions is transposed so that the actions from different peers are interleaved.
/// This makes sure that every peer has a chance to roll forward from the root of the tree.
pub fn generate_random_walks(seed: u64, generated_tree: &GeneratedTree, peers: &[Peer]) -> GeneratedActions {
    let mut actions_per_peer = BTreeMap::new();
    let mut rng = &mut SmallRng::seed_from_u64(seed);

    for peer in peers {
        random_walk(&mut rng, None, generated_tree.tree(), peer, &mut actions_per_peer);
    }

    // If more than 2 peers are required, duplicate peer 2 with the actions of peer 1
    if peers.len() > 2 {
        let peer_1 = &peers[0];
        let peer_2 = &peers[1];
        let mut duplicate_actions = vec![];
        for action in actions_per_peer.get(peer_1).cloned().unwrap_or_default() {
            duplicate_actions.push(action.set_peer(peer_2));
        }
        actions_per_peer.insert(peer_2.clone(), duplicate_actions);
    }

    // Truncate actions to avoid a final list of rollbacks to the root of the tree
    for actions in actions_per_peer.values_mut() {
        while let Some(Action::Rollback { .. }) = actions.last() {
            actions.pop();
        }
    }

    GeneratedActions { tree: generated_tree.clone(), actions_per_peer }
}

/// List of actions generated for a set of peers on a given tree of headers.
#[derive(Clone, PartialEq, Eq)]
pub struct GeneratedActions {
    tree: GeneratedTree,
    actions_per_peer: BTreeMap<Peer, Vec<Action>>,
}

impl GeneratedActions {
    pub fn set_actions(&mut self, actions: Vec<Action>) {
        let actions_per_peer = actions.into_iter().fold(BTreeMap::<Peer, Vec<Action>>::new(), |mut acc, action| {
            acc.entry(action.peer().clone()).or_default().push(action);
            acc
        });
        self.actions_per_peer = actions_per_peer;
    }

    pub fn get_anchor(&self) -> Header {
        self.tree.tree().value.clone()
    }

    pub fn generated_tree(&self) -> &GeneratedTree {
        &self.tree
    }

    pub fn best_chains(&self) -> Vec<Vec<Header>> {
        self.tree.best_chains()
    }

    pub fn actions_per_peer(&self) -> BTreeMap<Peer, Vec<Action>> {
        self.actions_per_peer.clone()
    }

    /// Transpose the actions per peer to interleave them
    /// so that we don't have all the actions from one peer first, then all the actions from another peer, etc...
    pub fn actions(&self) -> Vec<Action> {
        transpose(self.actions_per_peer.values()).into_iter().flatten().cloned().collect()
    }

    pub fn statistics(&self) -> GeneratedActionsStatistics {
        let fork_nodes = self.tree.fork_nodes();
        GeneratedActionsStatistics {
            tree_depth: self.tree.depth(),
            number_of_nodes: self.tree.nodes().len(),
            number_of_fork_nodes: fork_nodes.len(),
        }
    }
}

impl Debug for GeneratedActions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let lines = self.display_as_lines();
        for line in lines {
            writeln!(f, "{}", line)?;
        }
        Ok(())
    }
}

impl GeneratedActions {
    /// Return the actions as a list of lines, ready to be printed out.
    pub fn display_as_lines(&self) -> Vec<String> {
        let actions = self.actions();
        let mut result = vec![];
        result.push("ALL ACTIONS".to_string());
        for action in actions.iter() {
            result.push(Self::display_action(action))
        }

        result.push("BY PEER".to_string());
        for (peer, actions) in self.actions_per_peer.iter() {
            result.push(format!("\nActions from peer {}", peer));
            for action in actions.iter() {
                result.push(Self::display_action(action))
            }
        }

        result
    }

    fn display_action(action: &Action) -> String {
        GeneratedAction::from(action.clone()).to_string()
    }
}

impl GeneratedActions {
    pub fn as_json(&self) -> serde_json::Value {
        let actions_json: Vec<serde_json::Value> =
            self.actions().iter().map(|action| to_value(GeneratedAction::from(action.clone())).unwrap()).collect();

        serde_json::json!({
            "tree": self.generated_tree().as_json(),
            "messages": actions_json,
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

/// A single generated action formatted for display and serialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct GeneratedAction {
    message_type: String,
    src: String,
    hash: String,
    parent: String,
    slot: u64,
}

impl Display for GeneratedAction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&format!(
            "{message_type:<3} {src} {slot:>5} {hash:>6} (parent {parent_hash:>6})",
            message_type = self.message_type,
            src = self.src,
            slot = self.slot,
            hash = self.hash,
            parent_hash = self.parent,
        ))
    }
}

impl From<Action> for GeneratedAction {
    fn from(action: Action) -> Self {
        let message_type = match action {
            Action::RollForward { .. } => "FWD",
            Action::Rollback { .. } => "BCK",
        };
        let header_hash = action.hash();
        let header_parent_hash = action.parent_hash();
        let slot = action.slot();

        GeneratedAction {
            message_type: message_type.to_string(),
            src: action.peer().to_string(),
            hash: header_hash.to_string().chars().take(6).collect(),
            parent: header_parent_hash.map(|h| h.to_string().chars().take(6).collect()).unwrap_or("n/a".to_string()),
            slot: slot.as_u64(),
        }
    }
}

impl Shrinkable for GeneratedActions {
    fn complement(&self, from: usize, to: usize) -> Self
    where
        Self: Sized,
    {
        let mut complement: Vec<Action> = Vec::new();
        let actions = self.actions();

        complement.extend_from_slice(&actions[..to]);
        if from < self.len() {
            complement.extend_from_slice(&actions[from..]);
        };
        let mut generated_actions = self.clone();
        generated_actions.set_actions(complement);
        generated_actions
    }

    fn len(&self) -> usize {
        self.actions().len()
    }
}

pub struct GeneratedActionsStatistics {
    pub tree_depth: usize,
    pub number_of_nodes: usize,
    pub number_of_fork_nodes: usize,
}

impl Display for GeneratedActionsStatistics {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        for statistic in self.display_as_lines() {
            f.write_str(&format!("{}\n", statistic))?;
        }
        Ok(())
    }
}

impl GeneratedActionsStatistics {
    pub fn display_as_lines(&self) -> Vec<String> {
        let mut result = vec![];
        result.push(format!("Tree depth: {}", self.tree_depth));
        result.push(format!("Total number of nodes: {}", self.number_of_nodes));
        result.push(format!("Number of forks: {}", self.number_of_fork_nodes));
        result
    }
}

/// Generate a random list of actions, for a number of peers.
pub fn any_select_chains(depth: usize, peers: &[Peer]) -> impl Strategy<Value = GeneratedActions> {
    any_tree_of_headers(depth).prop_flat_map(move |generated_tree| {
        (1..u64::MAX).prop_map(move |seed| generate_random_walks(seed, &generated_tree, peers))
    })
}

/// Generate a random list of actions, for a fixed number of peers, with a given tree of headers.
pub fn any_select_chains_from_tree(tree: &GeneratedTree, peers: &[Peer]) -> impl Strategy<Value = GeneratedActions> {
    (1..u64::MAX).prop_map(move |seed| generate_random_walks(seed, tree, peers))
}

/// Transpose a list of rows into a list of columns (even if the rows have different lengths).
pub fn transpose<I, R, T>(rows: I) -> Vec<Vec<T>>
where
    I: IntoIterator<Item = R>,
    R: IntoIterator<Item = T>,
{
    let mut iterators: Vec<_> = rows.into_iter().map(|r| r.into_iter()).collect();
    let mut result: Vec<Vec<T>> = vec![];

    while !iterators.is_empty() {
        let mut column = Vec::with_capacity(iterators.len());
        let mut next_iterators = Vec::with_capacity(iterators.len());

        for mut iterator in iterators {
            if let Some(x) = iterator.next() {
                column.push(x);
                next_iterators.push(iterator);
            }
        }
        if !column.is_empty() {
            result.push(column);
        }
        iterators = next_iterators;
    }
    result
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use amaru_kernel::utils::string::ListToString;

    use super::*;
    use crate::simulator::data_generation::generate_tree_of_headers;

    #[test]
    fn transpose_works() {
        let rows = vec![vec![1, 2, 3], vec![4, 5], vec![6, 7, 8, 9], vec![10], vec![], vec![11, 12]];
        let expected = vec![vec![1, 4, 6, 10, 11], vec![2, 5, 7, 12], vec![3, 8], vec![9]];
        let result = transpose(rows);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_generate_random_walks() {
        let seed = 45;
        let tree = generate_tree_of_headers(seed, 10);
        let peers = (1..=3).map(|i| Peer::new(&format!("peer-{i}"))).collect::<Vec<_>>();
        let generated_actions = generate_random_walks(seed, &tree, &peers);
        let statistics = generated_actions.statistics();

        assert!(
            statistics.number_of_fork_nodes >= 2 && statistics.number_of_fork_nodes <= 4,
            "statistics.number_of_fork_nodes {}",
            statistics.number_of_fork_nodes
        );

        let actions_chains: Vec<String> = generated_actions
            .actions_per_peer
            .values()
            .map(|actions| {
                actions
                    .iter()
                    .map(|a: &Action| a.clone().set_peer(&Peer::new("unused")))
                    .collect::<Vec<_>>()
                    .list_to_string(",\n")
            })
            .collect();
        let actions_set: BTreeSet<String> = actions_chains.iter().cloned().collect();
        assert_eq!(
            actions_set.len(),
            2,
            "there must be at least 2 peers with the same list of actions\nall actions\n{}\n\nall actions as a set\n{}",
            actions_chains.list_to_string("\n\n"),
            actions_set.list_to_string("\n\n")
        );
    }
}
