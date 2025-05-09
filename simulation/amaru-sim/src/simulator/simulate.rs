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

use crate::echo::{EchoMessage, Envelope};

use proptest::prelude::*;
use proptest::test_runner::{Config, TestError, TestRunner};
use std::collections::{BTreeMap, BinaryHeap};
use std::fmt::Debug;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub struct Entry<Msg> {
    arrival_time: Instant,
    envelope: Envelope<Msg>,
}

impl<Msg: PartialEq> PartialOrd for Entry<Msg> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.arrival_time.cmp(&other.arrival_time))
    }
}

impl<Msg: PartialEq> Ord for Entry<Msg> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.arrival_time.cmp(&other.arrival_time)
    }
}

impl<Msg: PartialEq> Eq for Entry<Msg> {}

type NodeId = String;

// TODO: should be RK's handle to interact with a node
pub struct NodeHandle {
    handle: Box<dyn FnMut(Envelope<EchoMessage>) -> Result<Vec<Envelope<EchoMessage>>, String>>,
    close: Box<dyn FnMut()>,
}

pub fn pipe_node_handle<'a>(filepath: &'a Path, args: &'a [&'a str]) -> Result<NodeHandle, String> {
    let mut child = Command::new(filepath)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to create process: {}", e))?;
    let mut stdin = child.stdin.take().ok_or("Failed to take stdin")?;
    let mut stdout = child.stdout.take().ok_or("Failed to take stdout")?;

    let handle = Box::new(move |msg: Envelope<EchoMessage>| {
        writeln!(
            stdin,
            "{}",
            serde_json::to_string(&msg).map_err(|e| format!("Failed to encode JSON: {}", e))?
        )
        .map_err(|e| format!("Failed to write to child's stdin: {}", e))?;
        stdin
            .flush()
            .map_err(|e| format!("Failed to flush child's stdin: {}", e))?;

        let mut reader = BufReader::new(&mut stdout);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| format!("Failed to read from child's stdout: {}", e))?;

        serde_json::from_str(&line)
            .map(|msg: Envelope<EchoMessage>| vec![msg])
            .map_err(|e| format!("Failed to decode JSON: {}", e))
    });

    let close = Box::new(move || {
        child
            .kill()
            .map_err(|e| format!("Failed to terminate process: {}", e))
            .ok();
    });

    Ok(NodeHandle { handle, close })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Trace(Vec<Envelope<EchoMessage>>);

#[derive(Debug, PartialEq)]
pub enum Next {
    Done,
    Continue,
}

pub struct World {
    heap: BinaryHeap<Entry<EchoMessage>>,
    nodes: BTreeMap<NodeId, NodeHandle>,
    trace: Trace,
}

#[allow(dead_code)]
impl World {
    pub fn new(
        initial_messages: Vec<Entry<EchoMessage>>,
        node_handles: Vec<(NodeId, NodeHandle)>,
    ) -> Self {
        World {
            heap: BinaryHeap::from(initial_messages),
            nodes: node_handles.into_iter().collect(),
            trace: Trace(Vec::new()),
        }
    }

    /// Simulate a 'World' of interconnected nodes
    /// see https://github.com/pragma-org/simulation-testing/blob/main/blog/dist/04-simulation-testing-main-loop.md
    pub fn step_world(&mut self) -> Next {
        match self.heap.pop() {
            Some(Entry {
                arrival_time,
                envelope,
            }) =>
            // TODO: deal with time advance across all nodes
            // eg. run all nodes whose next action is ealier than msg's arrival time
            // and enqueue their output messages possibly bailing out and recursing
            {
                match self.nodes.get(&envelope.dest) {
                    Some(node) => match (node.handle)(envelope.clone()) {
                        Ok(outgoing) => {
                            let (client_responses, outputs): (
                                Vec<Envelope<EchoMessage>>,
                                Vec<Envelope<EchoMessage>>,
                            ) = outgoing
                                .into_iter()
                                .partition(|msg| msg.dest.starts_with("c"));
                            outputs
                                .iter()
                                .map(|envelope| Entry {
                                    arrival_time: arrival_time + Duration::from_millis(100),
                                    envelope: envelope.clone(),
                                })
                                .for_each(|msg| self.heap.push(msg));
                            if envelope.src.starts_with("c") {
                                self.trace.0.push(envelope);
                            }
                            client_responses
                                .iter()
                                .for_each(|msg| self.trace.0.push(msg.clone()));
                            Next::Continue
                        }
                        Err(err) => panic!("{}", err),
                    },
                    None => panic!("unknown destination node '{}'", envelope.dest),
                }
            }
            None => Next::Done,
        }
    }

    pub fn run_world(&mut self) -> Trace {
        while self.step_world() == Next::Continue {}
        self.trace.clone()
    }
}

pub fn simulate(
    config: Config,
    number_of_nodes: u8,
    spawn: fn() -> NodeHandle,
    generate_message: impl Strategy<Value = EchoMessage>,
    property: fn(Trace) -> Result<(), String>,
) {
    let mut runner = TestRunner::new(config);
    let generate_messages = prop::collection::vec(
        generate_message.prop_map(|msg| Entry {
            arrival_time: Instant::now(),
            envelope: Envelope {
                src: "c1".to_string(),
                dest: "n1".to_string(),
                body: msg,
            },
        }),
        0..20,
    );
    let result = runner.run(&generate_messages, |initial_messages| {
        let node_handles = (1..number_of_nodes)
            .map(|i| (format!("n{}", i), spawn()))
            .collect();
        let trace = World::new(initial_messages, node_handles).run_world();

        // XXX: How do we close the node handles here? The following doesn't work, because
        // node_handles has been moved in the line above, and we can also not clone it because of
        // dyn in the NodeHandle struct.
        // node_handles.into_iter().map(|(_node_id, node_handle)| node_handle.close());
        // node_handles
        //     .into_iter()
        //     .map(|(_node_id, node_handle)| node_handle.close());

        match property(trace) {
            Ok(()) => (),
            Err(reason) => assert!(false, "{}", reason),
        }
        Ok(())
    });
    match result {
        Ok(_) => (),
        Err(TestError::Fail(_, value)) => println!("Found minimal failing case: {:?}", value),
        Err(TestError::Abort(_)) => todo!(),
    }
}

#[cfg(test)]
mod tests {
    use crate::simulator::simulate::Trace;
    use crate::simulator::simulate::World;

    #[test]
    fn run_stops_when_no_message_to_process_is_left() {
        let mut world = World::new(Vec::new(), Vec::new());

        assert_eq!(world.run_world(), Trace(Vec::new()));
    }
}
