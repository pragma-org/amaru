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

// Spawn one pipeline for each node in the test;
// Generate client requests (and faults) with random arrival times and insert them in heap of messages to be delivered (the heap is ordered by arrival time);
// Pop the next client request from the heap of messages;
// Advance the time to the arrival time, of the popped message, on all nodes, potentially triggering timeouts;
// Call get_state to dump the current/pre-state on the receiving node;
// Deliver the message the receiving enqueue_msg (unless there's some network fault stopping it);
// Process the message using run_until_blocked and drain.collect all outgoing messages (storage effects will later have to be dealt with here as well);
// Dump the post-state and append the pre-state, post-state, incoming message and outgoing messages to the simulator's "trace";
// Assign random arrival times for the outgoing messages (this creates different message interleavings) and insert them back into the heap;
// Go to 3 and continue until heap is empty;
// Make assertions on the trace to ensure the execution was correct, if not, shrink and present minimal trace that breaks the assertion together with the seed that allows us to reproduce the execution.

use crate::echo::Envelope;

use super::sync::ChainSyncMessage;
use std::collections::{BTreeMap, BinaryHeap};
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq)]
pub struct Message {
    arrival_time: Instant,
    message: Envelope<ChainSyncMessage>,
}

impl PartialOrd for Message {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Message {
    fn cmp(&self, _other: &Self) -> std::cmp::Ordering {
        todo!()
    }
}

impl Eq for Message {}

type NodeId = String;

// TODO: should be RK's handle to interact with a node
trait Node {
    fn run(&self) -> Vec<Envelope<ChainSyncMessage>>;
    fn send(&self, message: ChainSyncMessage);
}

pub struct World {
    heap: BinaryHeap<Message>,
    nodes: BTreeMap<NodeId, Box<dyn Node>>,
}

#[derive(Debug, PartialEq)]
pub enum Next {
    Done,
    Continue,
}

#[allow(dead_code)]
impl World {
    pub fn new() -> Self {
        World {
            heap: BinaryHeap::new(),
            nodes: BTreeMap::new(),
        }
    }

    /// Simulate a 'World' of interconnected nodes
    /// see https://github.com/pragma-org/simulation-testing/blob/main/blog/dist/04-simulation-testing-main-loop.md
    pub fn step_world(&mut self) -> Next {
        match self.heap.pop() {
            Some(Message {
                arrival_time,
                message,
            }) =>
            // TODO: deal with time advance across all nodes
            // eg. run all nodes whose next action is ealier than msg's arrival time
            // and enqueue their output messages possibly bailing out and recursing
            {
                match self.nodes.get(&message.dest) {
                    Some(node) => {
                        node.send(message.body);
                        let outputs = node.run();
                        outputs
                            .iter()
                            .map(|envelope| Message {
                                arrival_time: arrival_time + Duration::from_millis(100),
                                message: envelope.clone(),
                            })
                            .for_each(|msg| self.heap.push(msg));
                        Next::Continue
                    }
                    None => todo!(),
                }
            }
            None => Next::Done,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::simulator::simulate::Next;
    use crate::simulator::simulate::World;

    #[test]
    fn step_stops_when_no_message_to_process_is_left() {
        let mut world = World::new();

        assert_eq!(world.step_world(), Next::Done);
    }
}
