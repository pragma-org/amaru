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
use pure_stage::Instant;
use serde::Serialize;
use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeMap, BinaryHeap};
use std::fmt::Debug;
use std::time::Duration;
use tracing::info;

pub struct World<Msg> {
    heap: BinaryHeap<Reverse<Entry<Msg>>>,
    nodes: BTreeMap<NodeId, NodeHandle<Msg>>,
    history: History<Msg>,
}

/// A `NodeHandle` is:
///
///  - An async function that sends an Envelope<Msg> to a node and returns a list of Envelope<Msg>
///    as the result of processing that message (Envelope holds source/destination values representing node ids).
///  - An async function to shutdown the node gracefully.
///
pub struct NodeHandle<Msg> {
    handle: Box<dyn FnMut(Envelope<Msg>) -> Result<Vec<Envelope<Msg>>, anyhow::Error>>,
    close: Box<dyn FnMut()>,
}

impl<Msg> NodeHandle<Msg> {
    pub fn new<
        F: FnMut(Envelope<Msg>) -> Result<Vec<Envelope<Msg>>, anyhow::Error> + 'static,
        G: FnMut() + 'static,
    >(
        handle: F,
        close: G,
    ) -> Self {
        Self {
            handle: Box::new(handle),
            close: Box::new(close),
        }
    }
}

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

#[derive(Debug, Clone, PartialEq)]
pub struct History<Msg>(pub Vec<Envelope<Msg>>);

#[derive(Debug, PartialEq)]
pub enum Next {
    Done,
    Continue,
    Panic(String),
}

impl<Msg: PartialEq + Clone + Debug> World<Msg> {
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

    /// Simulate a 'World' of interconnected nodes
    /// see https://github.com/pragma-org/simulation-testing/blob/main/blog/dist/04-simulation-testing-main-loop.md
    pub fn step_world(&mut self) -> Next {
        match self.heap.pop() {
            Some(Reverse(Entry {
                arrival_time,
                envelope,
            })) =>
            // TODO: deal with time advance across all nodes
            // eg. run all nodes whose next action is ealier than msg's arrival time
            // and enqueue their output messages possibly bailing out and recursing
            {
                info!(msg = ?envelope, arrival = ?arrival_time, heap = ?self.heap, "stepping");
                match self.nodes.get_mut(&envelope.dest) {
                    Some(node) => match (node.handle)(envelope.clone()) {
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
                            if envelope.src.starts_with("c") {
                                self.history.0.push(envelope);
                            }
                            client_responses
                                .iter()
                                .for_each(|msg| self.history.0.push(msg.clone()));
                            Next::Continue
                        }
                        Err(err) => Next::Panic(format!("{}", err)),
                    },
                    None => panic!("unknown destination node '{}'", envelope.dest),
                }
            }
            None => Next::Done,
        }
    }

    pub fn run_world(&mut self) -> Result<&[Envelope<Msg>], (String, &[Envelope<Msg>])> {
        info!("run_world");
        let prev = self.history.0.len();
        let mut next = Next::Continue;
        while next == Next::Continue {
            next = self.step_world()
        }
        match next {
            Next::Panic(reason) => Err((reason, &self.history.0[prev..])),
            Next::Continue => unreachable!(),
            Next::Done => Ok(&self.history.0[prev..]),
        }
    }
}

impl<Msg> Drop for World<Msg> {
    fn drop(&mut self) {
        self.nodes
            .values_mut()
            .for_each(|node_handle| (node_handle.close)());
    }
}
