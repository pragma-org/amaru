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

use std::collections::BTreeSet;

use anyhow::anyhow;
use pure_stage::{Resources, simulation::RandStdRng};

use crate::tests::node::Node;

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
    ///
    /// Phase 1: Enqueue actions and run effects with time advancement until all
    /// actions are consumed.
    /// Phase 2 (drain): Continue running effects (with time advancement) but
    /// without enqueueing new actions. This allows in-flight work to complete:
    /// block fetches, chain relay to downstream, retries, etc.
    /// The drain stops when no node has processed any effect for a sustained
    /// period, indicating all cross-node communication has settled.
    pub fn run(&mut self, rng: &mut RandStdRng) {
        let max_steps = 10_000; // safety limit

        // Phase 1: Run with action enqueueing until all actions consumed
        for step in 0..max_steps {
            for node in self.nodes.iter_mut() {
                node.enqueue_pending_action();
                node.advance_inputs();
            }

            if self.nodes.iter().all(|n| !n.has_pending_actions()) {
                tracing::info!("All actions consumed at step {step}, entering drain phase");
                break;
            }

            let Some(node) = self.pick_random_active_node(rng) else {
                tracing::info!("All nodes terminated at step {step}");
                return;
            };
            node.run_effect();
        }

        // Phase 2: Drain remaining effects
        self.drain(rng);
    }

    /// Drain remaining effects after all actions have been consumed.
    ///
    /// Runs effects identically to Phase 1 but without
    ///  - Time advancement for sleeping nodes (since we want to avoid keep-alive make the system run forever).
    ///  - Enqueueing new actions (since everything has been enqueued before).
    ///
    /// Stops when no node has any effects to run.
    ///
    fn drain(&mut self, rng: &mut RandStdRng) {
        // Bound the drain loop in case it does not terminate
        let max_drain_steps = 10_000;
        for step in 0..max_drain_steps {
            for node in self.nodes.iter_mut() {
                node.advance_inputs();
            }

            let Some(node) = self.pick_random_runnable_node(rng) else {
                tracing::info!("drain[{step}]: all nodes terminated or have no effects to run");
                return;
            };
            node.run_effect();
        }
        tracing::info!("Drain phase completed after {max_drain_steps} steps");
    }

    pub fn get_node_under_test(&mut self) -> anyhow::Result<&mut Node> {
        self.nodes.iter_mut().find(|node| node.is_node_under_test()).ok_or(anyhow!("No node under test found"))
    }

    pub fn to_vec(self) -> Vec<Node> {
        self.nodes
    }

    pub fn nodes(&self) -> &Vec<Node> {
        &self.nodes
    }

    /// Pick a random non-terminated node
    fn pick_random_active_node(&mut self, rng: &mut RandStdRng) -> Option<&mut Node> {
        self.pick_random_node(rng, |n| !n.is_terminated())
    }

    /// Pick a random non-terminated node with runnable effects
    fn pick_random_runnable_node(&mut self, rng: &mut RandStdRng) -> Option<&mut Node> {
        self.pick_random_node(rng, |n| !n.is_terminated() && n.has_runnable_effects())
    }

    fn pick_random_node<F>(&mut self, rng: &mut RandStdRng, mut predicate: F) -> Option<&mut Node>
    where
        F: FnMut(&Node) -> bool,
    {
        let indices: Vec<usize> = self.nodes.iter().enumerate().filter(|(_, n)| predicate(n)).map(|(i, _)| i).collect();

        if indices.is_empty() {
            return None;
        }

        let idx = indices[rng.random_range(0..indices.len())];
        Some(&mut self.nodes[idx])
    }
}
