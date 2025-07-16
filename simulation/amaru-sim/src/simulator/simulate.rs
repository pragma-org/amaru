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
// Dump the post-state and append the pre-state, post-state, incoming message and outgoing messages to the simulator's "history";
// Assign random arrival times for the outgoing messages (this creates different message interleavings) and insert them back into the heap;
// Go to 3 and continue until heap is empty;
// Make assertions on the history to ensure the execution was correct, if not, shrink and present minimal history that breaks the assertion together with the seed that allows us to reproduce the execution.

use crate::echo::{EchoMessage, Envelope};
use anyhow::anyhow;
use parking_lot::Mutex;
use proptest::{
    prelude::*,
    test_runner::{Config, TestError, TestRunner},
};
use pure_stage::trace_buffer::TraceBuffer;
use pure_stage::StageRef;
use pure_stage::{simulation::SimulationRunning, Instant, Receiver};
use serde::Serialize;
use std::cmp::Ordering;
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BinaryHeap},
    fmt::Debug,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};
use tracing::info;

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

type NodeId = String;

pub struct NodeHandle<Msg> {
    handle: Box<dyn FnMut(Envelope<Msg>) -> Result<Vec<Envelope<Msg>>, anyhow::Error>>,
    close: Box<dyn FnMut()>,
}

pub fn pure_stage_node_handle<Msg, St>(
    mut rx: Receiver<Envelope<Msg>>,
    stage: StageRef<Envelope<Msg>, St>,
    mut running: SimulationRunning,
) -> anyhow::Result<NodeHandle<Msg>>
where
    Msg: PartialEq + Send + Debug + serde::Serialize + serde::de::DeserializeOwned + 'static,
    St: 'static,
{
    let handle = Box::new(move |msg: Envelope<Msg>| {
        info!(msg = ?msg, "enqueuing");
        running.enqueue_msg(&stage, [msg]);
        running.run_until_blocked().assert_idle();
        Ok(rx.drain().collect::<Vec<_>>())
    });

    let close = Box::new(move || ());

    Ok(NodeHandle { handle, close })
}

pub fn pipe_node_handle(filepath: &Path, args: &[&str]) -> anyhow::Result<NodeHandle<EchoMessage>> {
    let mut child = Command::new(filepath)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("Failed to create process: {}", e))?;
    let mut stdin = child.stdin.take().ok_or(anyhow!("Failed to take stdin"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or(anyhow!("Failed to take stdout"))?;

    let handle = Box::new(move |msg: Envelope<EchoMessage>| {
        let json =
            serde_json::to_string(&msg).map_err(|e| anyhow!("Failed to encode JSON: {}", e))?;
        println!("About to write: {}", json);
        writeln!(stdin, "{}", json)
            .map_err(|e| anyhow!("Failed to write to child's stdin: {}", e))?;
        stdin
            .flush()
            .map_err(|e| anyhow!("Failed to flush child's stdin: {}", e))?;

        let mut reader = BufReader::new(&mut stdout);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| anyhow!("Failed to read from child's stdout: {}", e))?;

        println!("Just read: {}", &line);
        serde_json::from_str(&line)
            // TODO: Read more than one message? Either make SUT send one message
            // per line and end by a termination token, or make write a JSON array
            // of messages?
            .map(|msg: Envelope<EchoMessage>| vec![msg])
            .map_err(|e| anyhow!("Failed to decode JSON: {}", e))
    });

    let close = Box::new(move || {
        child
            .kill()
            .map_err(|e| anyhow!("Failed to terminate process: {}", e))
            .ok();
    });

    Ok(NodeHandle { handle, close })
}

#[derive(Debug, Clone, PartialEq)]
pub struct History<Msg>(pub Vec<Envelope<Msg>>);

#[derive(Debug, PartialEq)]
pub enum Next {
    Done,
    Continue,
}

pub struct World<Msg> {
    heap: BinaryHeap<Reverse<Entry<Msg>>>,
    nodes: BTreeMap<NodeId, NodeHandle<Msg>>,
    history: History<Msg>,
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
                        Err(err) => panic!("{}", err),
                    },
                    None => panic!("unknown destination node '{}'", envelope.dest),
                }
            }
            None => Next::Done,
        }
    }

    pub fn run_world(&mut self) -> &[Envelope<Msg>] {
        info!("run_world");
        let prev = self.history.0.len();
        while self.step_world() == Next::Continue {}
        &self.history.0[prev..]
    }
}

impl<Msg> Drop for World<Msg> {
    fn drop(&mut self) {
        self.nodes
            .values_mut()
            .for_each(|node_handle| (node_handle.close)());
    }
}

pub fn simulate<Msg, F>(
    config: Config,
    seed: u64,
    number_of_nodes: u8,
    spawn: F,
    generate_entries: impl Strategy<Value = Vec<Reverse<Entry<Msg>>>>,
    property: impl Fn(History<Msg>) -> Result<(), String>,
    trace_buffer: Arc<parking_lot::Mutex<TraceBuffer>>,
    persist_on_success: bool,
) where
    Msg: Debug + PartialEq + Clone + Serialize,
    F: Fn() -> NodeHandle<Msg>,
{
    let mut runner = TestRunner::new(config);
    let result = runner.run(&generate_entries, |initial_messages| {
        let node_handles: Vec<_> = (1..=number_of_nodes)
            .map(|i| (format!("n{}", i), spawn()))
            .collect();

        let mut world = World::new(initial_messages, node_handles);
        let history = world.run_world();

        match property(History(history.to_vec())) {
            Ok(()) => (),
            Err(reason) => prop_assert!(false, "{}", reason),
        }
        Ok(())
    });
    match result {
        Ok(_) => {
            if persist_on_success {
                persist_schedule_(Path::new("."), "success", trace_buffer)
            }
        }
        Err(TestError::Fail(what, entries)) => {
            let mut err = String::new();
            entries
                .into_iter()
                .for_each(|entry| err += &format!("  {:?}\n", entry.0.envelope));
            persist_schedule_(Path::new("."), "failure", trace_buffer);
            panic!(
                "Found minimal failing case:\n\n{}\nError message:\n\n  {}\n\nSeed: {}\n",
                err, what, seed
            )
        }
        Err(TestError::Abort(e)) => panic!("Test aborted: {}", e),
    }
}

fn persist_schedule_(dir: &Path, prefix: &str, trace_buffer: Arc<Mutex<TraceBuffer>>) {
    match persist_schedule(dir, prefix, trace_buffer) {
        Err(err) => eprintln!("{}", err),
        Ok(path) => eprintln!("Saved schedule: {:?}", path),
    }
}

fn persist_schedule(
    dir: &Path,
    prefix: &str,
    trace_buffer: Arc<Mutex<TraceBuffer>>,
) -> Result<PathBuf, anyhow::Error> {
    if trace_buffer.lock().is_empty() {
        return Err(anyhow::anyhow!("empty schedule"));
    }

    let now = SystemTime::now();

    let filename = format!(
        "{}-{}.schedule",
        prefix,
        now.duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );
    let path = dir.join(filename);

    let mut file = File::create(&path)?;
    for bytes in trace_buffer.lock().iter() {
        file.write_all(bytes)?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use pure_stage::{simulation::SimulationBuilder, StageGraph, Void};

    #[test]
    fn run_stops_when_no_message_to_process_is_left() {
        let mut world: World<EchoMessage> = World::new(Vec::new(), Vec::new());

        assert_eq!(world.run_world(), &Vec::new());
    }

    #[test]
    #[should_panic]
    fn simulate_pure_stage_echo() {
        #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct State(u64, StageRef<Envelope<EchoMessage>, Void>);

        let config = Config::default();
        let seed = 42;

        let number_of_nodes = 1;

        let spawn: fn() -> NodeHandle<EchoMessage> = || {
            println!("*** Spawning node!");
            let mut network = SimulationBuilder::default();
            let stage = network.stage(
                "echo",
                async |mut state: State, msg: Envelope<EchoMessage>, eff| {
                    if let EchoMessage::Echo { msg_id, echo } = &msg.body {
                        state.0 += 1;
                        // Insert a bug every 5 messages.
                        let echo_response = if state.0 % 5 == 0 {
                            echo.to_string().to_uppercase()
                        } else {
                            echo.to_string()
                        };
                        let reply = Envelope {
                            src: msg.dest,
                            dest: msg.src,
                            body: EchoMessage::EchoOk {
                                msg_id: state.0,
                                in_reply_to: *msg_id,
                                echo: echo_response,
                            },
                        };
                        println!(" ==> {:?}", reply);
                        eff.send(&state.1, reply).await;
                        Ok(state)
                    } else {
                        panic!("Got a message that wasn't an echo: {:?}", msg.body)
                    }
                },
            );
            let (output, rx) = network.output("output", 10);
            let stage = network.wire_up(stage, State(0, output.without_state()));
            let rt = tokio::runtime::Runtime::new().unwrap();
            let running = network.run(rt.handle().clone());

            pure_stage_node_handle(rx, stage.without_state(), running).unwrap()
        };
        let now = Instant::at_offset(Duration::from_secs(0));
        let generate_messages = prop::collection::vec(
            (0..128u8).prop_map(|i| {
                Reverse(Entry {
                    arrival_time: now,
                    envelope: Envelope {
                        src: "c1".to_string(),
                        dest: "n1".to_string(),
                        body: EchoMessage::Echo {
                            msg_id: 0,
                            echo: format!("Please echo {}", i),
                        },
                    },
                })
            }),
            0..20,
        );
        simulate(
            config,
            seed,
            number_of_nodes,
            spawn,
            generate_messages,
            ECHO_PROPERTY,
            TraceBuffer::new_shared(0, 0),
            false,
        )
    }

    // TODO: Take response time into account.
    const ECHO_PROPERTY: fn(History<EchoMessage>) -> Result<(), String> = |history: History<
        EchoMessage,
    >| {
        for (index, msg) in history
            .0
            .iter()
            .enumerate()
            .filter(|(_index, msg)| msg.src.starts_with("c"))
        {
            if let EchoMessage::Echo { msg_id, echo } = &msg.body {
                let response = history.0.split_at(index + 1).1.iter().find(|resp| {
                        resp.dest == msg.src
                            && matches!(&resp.body, EchoMessage::EchoOk { in_reply_to, echo: resp_echo, .. }
                                if in_reply_to == msg_id && resp_echo == echo)
                    });
                if response.is_none() {
                    let mut err = String::new();
                    err += &format!(
                        "No matching response found for echo request:\n    {:?}\n\nHistory:\n",
                        msg
                    );
                    for envelope in history.0 {
                        err += &format!("  {envelope:?}\n");
                    }
                    return Err(err);
                }
            }
        }
        Ok(())
    };

    // This shows how we can test external binaries. The test is disabled because building and
    // locating a binary on CI, across all platforms, is annoying.
    #[allow(dead_code)]
    #[ignore]
    fn blackbox_test_echo() {
        let config = proptest::test_runner::Config {
            cases: 100,
            verbose: 1,
            ..Default::default()
        };

        let seed = 42;

        let number_of_nodes = 1;
        let spawn: fn() -> NodeHandle<EchoMessage> = || {
            pipe_node_handle(Path::new("../../target/debug/echo"), &[]).expect("node handle failed")
        };
        let now = Instant::at_offset(Duration::from_secs(0));
        let generate_messages = prop::collection::vec(
            (0..128u8).prop_map(|i| {
                Reverse(Entry {
                    arrival_time: now,
                    envelope: Envelope {
                        src: "c1".to_string(),
                        dest: "n1".to_string(),
                        body: EchoMessage::Echo {
                            msg_id: 0,
                            echo: format!("Please echo {}", i),
                        },
                    },
                })
            }),
            0..20,
        );
        simulate(
            config,
            seed,
            number_of_nodes,
            spawn,
            generate_messages,
            ECHO_PROPERTY,
            TraceBuffer::new_shared(0, 0),
            false,
        )
    }

    #[test]
    fn persist_empty_schedule() {
        let schedule = TraceBuffer::new_shared(0, 0);
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().to_path_buf();
        let result = persist_schedule(&path, "test", schedule);
        assert!(result.is_err())
    }

    #[test]
    fn persist_non_empty_schedule() {
        let schedule = TraceBuffer::new_shared(3, 128);
        let now = Instant::at_offset(Duration::from_secs(0));
        schedule.lock().push_clock(now);
        schedule.lock().push_clock(now);
        schedule.lock().push_clock(now);

        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().to_path_buf();

        let result = persist_schedule(&path, "test", schedule);
        assert!(result.is_ok(), "{:?}", result);

        let file_size = fs::metadata(result.unwrap()).unwrap().len();
        assert_eq!(file_size, 63)
    }
}
