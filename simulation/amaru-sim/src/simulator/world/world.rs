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
use crate::simulator::NodeHandle;
use crate::simulator::world::world::Next::{Continue, Done, Panic};
use pure_stage::Instant;
use serde::Serialize;
use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeMap, BinaryHeap};
use std::fmt::Debug;
use std::time::Duration;
use tracing::info;

/// This data structure represents a simulated 'world' of interconnected nodes.
/// Nodes are identified by string ids:
///
/// - Node ids starting with 'c' are "client nodes". They are not part of the system being tested.
/// - Node ids that don't start with 'c' are the "system nodes". They are form the system being tested.
///
/// The World data type holds:
///
/// - A map of node ids to node handles (async functions that process messages sent to that node).
/// - A priority queue of messages to be delivered to nodes at specific times.
/// - A history of messages sent to/from client nodes
///
pub struct World<Msg> {
    heap: BinaryHeap<Reverse<Entry<Msg>>>,
    nodes: BTreeMap<NodeId, NodeHandle<Msg>>,
    history: History<Msg>,
}

/// An `Entry` represents a message (Envelope<Msg>) scheduled to be delivered at a specific time (Instant).
/// Entries are ordered by their arrival_time to facilitate priority queueing.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Entry<Msg> {
    pub arrival_time: Instant,
    pub envelope: Envelope<Msg>,
}

impl<Msg: PartialEq> PartialOrd for Entry<Msg> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<Msg: PartialEq> Ord for Entry<Msg> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.arrival_time.cmp(&other.arrival_time)
    }
}

impl<Msg: PartialEq> Eq for Entry<Msg> {}

pub type NodeId = String;

/// A `History` records all messages sent to/from client nodes.
#[derive(Debug, Clone, PartialEq)]
pub struct History<Msg>(pub Vec<Envelope<Msg>>);

impl<Msg: PartialEq + Clone + Debug> World<Msg> {
    /// Create a new World with initial messages and node handles.
    pub fn new(
        initial_messages: Vec<Reverse<Entry<Msg>>>,
        node_handles: Vec<(NodeId, NodeHandle<Msg>)>,
    ) -> Self {
        World {
            heap: BinaryHeap::from(initial_messages),
            nodes: node_handles.into_iter().collect(),
            history: History(Vec::new()),
        }
    }

    /// Run the simulation until there are no more messages to process or a panic occurs.
    /// Returns either the history of messages processed since the last run or an error with the reason
    /// for the panic and the history of messages processed until the panic.
    pub fn run_world(&mut self) -> Result<&[Envelope<Msg>], (String, &[Envelope<Msg>])> {
        info!("run_world");
        let prev = self.history.0.len();
        let mut next = Continue;
        while next == Continue {
            next = self.step_world()
        }
        match next {
            Panic(reason) => Err((reason, &self.history.0[prev..])),
            Continue => unreachable!(),
            Done => Ok(&self.history.0[prev..]),
        }
    }

    /// Simulate a 'World' of interconnected nodes
    /// see https://github.com/pragma-org/simulation-testing/blob/main/blog/dist/04-simulation-testing-main-loop.md
    fn step_world(&mut self) -> Next {
        match self.heap.pop() {
            Some(Reverse(Entry {
                arrival_time,
                envelope,
            })) =>
            // TODO: deal with time advance across all nodes
            // eg. run all nodes whose next action is earlier than msg's arrival time
            // and enqueue their output messages possibly bailing out and recursing
            {
                info!(msg = ?envelope, arrival = ?arrival_time, heap = ?self.heap, "stepping");

                match self.nodes.get_mut(&envelope.dest) {
                    Some(node) => match node.handle_msg(envelope.clone()) {
                        Ok(outgoing) => {
                            info!(outgoing = ?outgoing, "outgoing");
                            let (client_responses, outputs): (
                                Vec<Envelope<Msg>>,
                                Vec<Envelope<Msg>>,
                            ) = outgoing
                                .into_iter()
                                .partition(|msg| msg.dest.starts_with("c"));
                            outputs
                                .iter()
                                .map(|envelope| Entry {
                                    arrival_time: arrival_time + Duration::from_millis(100),
                                    envelope: envelope.clone(),
                                })
                                .for_each(|msg| self.heap.push(Reverse(msg)));
                            if envelope.is_client_message() {
                                self.history.0.push(envelope);
                            }
                            client_responses
                                .iter()
                                .for_each(|msg| self.history.0.push(msg.clone()));
                            Continue
                        }
                        Err(err) => Panic(format!("{}", err)),
                    },
                    None => panic!("unknown destination node '{}'", envelope.dest),
                }
            }
            None => Done,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Next {
    Done,
    Continue,
    Panic(String),
}

impl<Msg> Drop for World<Msg> {
    fn drop(&mut self) {
        self.nodes
            .values_mut()
            .for_each(|node_handle| (node_handle.close)());
    }
}
