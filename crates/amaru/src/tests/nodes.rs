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
use pure_stage::{Resources, simulation::RandStdRng, trace_buffer::TraceEntry};

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
                node.enqueue_pending_event();
                node.advance_inputs();
            }

            if self.nodes.iter().all(|n| !n.has_waiting_events()) {
                tracing::info!("All events consumed at step {step}, entering drain phase");
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
    /// Runs effects identically to Phase 1 but without enqueueing new actions.
    ///
    /// Stops when the system reaches a fixed point: all nodes are quiescent, we advance
    /// to the earliest global wakeup, and the system becomes quiescent again without any
    /// runnable work in between. This lets delayed retries run while still terminating
    /// once only background wait/sleep activity remains.
    ///
    fn drain(&mut self, rng: &mut RandStdRng) {
        // Bound the drain loop in case it does not terminate
        let max_drain_steps = 1_000_000;
        // This keeps track of the nodes trace buffers lengths in order
        // to examine only new trace entries since the last wake-up and check if only
        // transport activity was performed.
        let mut trace_marks: Option<Vec<usize>> = None;
        for step in 0..max_drain_steps {
            for node in self.nodes.iter_mut() {
                node.advance_inputs();
            }

            if let Some(node) = self.pick_random_runnable_node(rng) {
                node.run_effect();
                continue;
            }

            if let Some(marks) = trace_marks.take() {
                if self.only_transport_activity_since(&marks) {
                    tracing::info!("drain[{step}]: only transport activity after wakeup cycle");
                    return;
                } else {
                    tracing::info!("drain[{step}]: non-transport activity detected, continuing");
                    continue;
                }
            }

            let Some(next_wakeup) = self.next_global_wakeup() else {
                tracing::info!("drain[{step}]: quiescent");
                return;
            };

            trace_marks = Some(self.trace_lengths());

            let mut woke_any = false;
            for node in self.nodes.iter_mut().filter(|node| !node.is_terminated()) {
                woke_any |= node.advance_to_wakeup(next_wakeup);
            }

            if !woke_any {
                tracing::info!("drain[{step}]: no work after advancing to next wakeup");
                return;
            }
        }
        tracing::info!("Drain phase completed after {max_drain_steps} steps");
    }

    fn trace_lengths(&self) -> Vec<usize> {
        self.nodes.iter().map(|node| node.trace_buffer().lock().len()).collect()
    }

    fn next_global_wakeup(&self) -> Option<pure_stage::Instant> {
        self.nodes.iter().filter(|node| !node.is_terminated()).filter_map(Node::next_wakeup).min()
    }

    fn only_transport_activity_since(&self, marks: &[usize]) -> bool {
        self.nodes.iter().zip(marks).all(|(node, mark)| {
            node.trace_buffer()
                .lock()
                .iter_entries()
                .skip(*mark)
                .all(|(_, entry)| trace_entry_is_transport_activity(&entry))
        })
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
        self.pick_random_node(rng, |n| !n.is_terminated() && (n.has_runnable_effects() || n.has_effects()))
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

/// Return true if a trace entry represents some transport activity
/// (e.g. sending/receiving messages, scheduling keeepalive timer) rather than internal computation.
fn trace_entry_is_transport_activity(entry: &TraceEntry) -> bool {
    let Some(name) = entry.name() else {
        return true;
    };
    let prefix = name.as_str().split('-').next().unwrap_or(name.as_str());
    matches!(prefix, "keepalive" | "mux" | "reader" | "writer")
}
