// Copyright 2024 PRAGMA
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

use crate::tests::node::Node;
use anyhow::anyhow;
use pure_stage::Resources;
use pure_stage::simulation::RandStdRng;
use std::collections::BTreeSet;

/// List of nodes that are part of a simulation:
///
///  - One node is the node under test
///  - Other nodes are
///     - Upstream nodes, sending chainsync messages, receiving txsubmission messages.
///     - Downstream nodes, receiving chainsync messages, sending txsubmission messages
///
/// The nodes need first to be initialized in order for connections to be established according
/// to the mini-protocols. Then the run method can be called to randomly execute nodes during a test
/// with a maximum number of steps (otherwise the nodes could execute forever by just respecting the
/// keepalive miniprotocol).
///
#[derive(Debug)]
pub struct Nodes {
    nodes: Vec<Node>,
}

impl<'a> IntoIterator for &'a Nodes {
    type Item = &'a Node;
    type IntoIter = std::slice::Iter<'a, Node>;

    fn into_iter(self) -> Self::IntoIter {
        self.nodes.iter()
    }
}

impl<'a> IntoIterator for &'a mut Nodes {
    type Item = &'a mut Node;
    type IntoIter = std::slice::IterMut<'a, Node>;

    fn into_iter(self) -> Self::IntoIter {
        self.nodes.iter_mut()
    }
}

impl Nodes {
    /// Create a list of Nodes for the simulation
    pub fn new(nodes: Vec<Node>) -> Self {
        Self { nodes }
    }

    /// Return all the resources used by the nodes
    pub fn resources(&self) -> Vec<Resources> {
        self.nodes.iter().map(|n| n.resources()).collect()
    }

    /// Initialize nodes by running until the chainsync protocol is registered on all nodes.
    /// This uses a breakpoint to detect when the node under test is ready to receive chainsync messages.
    pub fn initialize(&mut self, rng: &mut RandStdRng) {
        let mut initialized_nodes = BTreeSet::<String>::new();
        loop {
            // First all the nodes for inputs
            for node in self.nodes.iter_mut() {
                node.advance_inputs();
            }

            // Pick a random active node (including already-initialized ones, as they may need to
            // continue processing for other nodes to make progress)
            let Some(node) = self.pick_random_active_node(rng) else {
                tracing::info!("All nodes terminated");
                break;
            };

            // take one execution step
            node.run_until_blocked();

            // If all the nodes are initialized, we can start the test run
            if node.is_initialized() && !initialized_nodes.contains(node.node_id()) {
                initialized_nodes.insert(node.node_id().to_string());
            }
            if initialized_nodes.len() == self.nodes.len() {
                break;
            }
        }
    }

    /// Run nodes with fine-grained interleaving.
    /// Each step runs exactly one effect on a randomly selected node.
    pub fn run(&mut self, rng: &mut RandStdRng, max_steps: usize) {
        for step in 0..max_steps {
            for node in self.nodes.iter_mut() {
                // Enqueue one pending action if available
                node.enqueue_pending_action();
                // Receive external effects results or input messages
                node.advance_inputs();
            }

            let Some(node) = self.pick_random_active_node(rng) else {
                tracing::info!("All nodes terminated at step {step}");
                return;
            };
            node.run_effect();
        }
        tracing::info!("Nodes ran for {max_steps} steps");
    }

    pub fn get_node_under_test(&mut self) -> anyhow::Result<&mut Node> {
        self.nodes
            .iter_mut()
            .find(|node| node.is_node_under_test())
            .ok_or(anyhow!("No node under test found"))
    }

    pub fn to_vec(self) -> Vec<Node> {
        self.nodes
    }

    pub fn nodes(&self) -> &Vec<Node> {
        &self.nodes
    }

    /// Pick a random non-terminated node from the list.
    fn pick_random_active_node(&mut self, rng: &mut RandStdRng) -> Option<&mut Node> {
        let active_indices: Vec<usize> = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| !n.is_terminated())
            .map(|(i, _)| i)
            .collect();

        if active_indices.is_empty() {
            return None;
        }

        let idx = active_indices[rng.random_range(0..active_indices.len())];
        Some(&mut self.nodes[idx])
    }
}
