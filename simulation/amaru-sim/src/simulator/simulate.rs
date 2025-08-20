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
use crate::simulator::shrink::shrink;
use anyhow::anyhow;
use parking_lot::Mutex;
use pure_stage::trace_buffer::TraceBuffer;
use pure_stage::StageRef;
use pure_stage::{simulation::SimulationRunning, Instant, Receiver};
use rand::{rngs::StdRng, SeedableRng};
use serde::Serialize;
use std::cmp::Ordering;
use std::fs::File;
use std::panic;
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
use tracing::{info, warn};

pub struct SimulateConfig {
    pub number_of_tests: u32,
    pub seed: u64,
    pub number_of_nodes: u8,
    pub disable_shrinking: bool,
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
    Panic(String),
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

fn run_test<Msg: Debug + PartialEq + Clone, F: Fn() -> NodeHandle<Msg>>(
    number_of_nodes: u8,
    spawn: F,
    property: impl Fn(&History<Msg>) -> Result<(), String>,
) -> impl Fn(&[Reverse<Entry<Msg>>]) -> (History<Msg>, Result<(), String>) {
    move |entries| {
        let node_handles: Vec<_> = (1..=number_of_nodes)
            .map(|i| (format!("n{}", i), spawn()))
            .collect();

        let mut world = World::new(entries.to_vec(), node_handles);

        match world.run_world() {
            Ok(history) => {
                let history = History(history.to_vec());
                let result = property(&history);
                (history, result)
            }
            Err((reason, history)) => (History(history.to_vec()), Err(reason)),
        }
    }
}

pub fn simulate<Msg, F>(
    config: SimulateConfig,
    spawn: F,
    generator: impl Fn(&mut StdRng) -> Vec<Reverse<Entry<Msg>>>,
    property: impl Fn(&History<Msg>) -> Result<(), String>,
    trace_buffer: Arc<parking_lot::Mutex<TraceBuffer>>,
    persist_on_success: bool,
) where
    Msg: Debug + PartialEq + Clone + Serialize,
    F: Fn() -> NodeHandle<Msg>,
{
    let mut rng = StdRng::seed_from_u64(config.seed);

    for test_number in 1..=config.number_of_tests {
        let entries: Vec<Reverse<Entry<Msg>>> = generator(&mut rng);

        let test = run_test(config.number_of_nodes, &spawn, &property);
        match test(&entries) {
            (history, Err(reason)) => {
                if config.disable_shrinking {
                    let number_of_shrinks = 0;
                    display_failure(
                        test_number,
                        config.seed,
                        entries,
                        number_of_shrinks,
                        history,
                        trace_buffer.clone(),
                        reason,
                    );
                } else {
                    let (shrunk_entries, (shrunk_history, result), number_of_shrinks) =
                        shrink(test, entries, |result| result.1 == Err(reason.clone()));
                    assert_eq!(Err(reason.clone()), result);
                    display_failure(
                        test_number,
                        config.seed,
                        shrunk_entries,
                        number_of_shrinks,
                        shrunk_history,
                        trace_buffer.clone(),
                        reason,
                    );
                }
                break;
            }
            (_history, Ok(())) => continue,
        }
    }
    if persist_on_success {
        persist_schedule_(Path::new("."), "success", trace_buffer)
    }
    info!("Success! ({} tests passed.)", config.number_of_tests);
}

fn display_failure<Msg: Debug>(
    test_number: u32,
    seed: u64,
    entries: Vec<Reverse<Entry<Msg>>>,
    number_of_shrinks: u32,
    history: History<Msg>,
    trace_buffer: Arc<parking_lot::Mutex<TraceBuffer>>,
    reason: String,
) {
    let mut test_case = String::new();
    entries
        .into_iter()
        .for_each(|entry| test_case += &format!("  {:?}\n", entry.0.envelope));
    let mut history_string = String::new();
    history
        .0
        .into_iter()
        .enumerate()
        .for_each(|(index, envelope)| {
            history_string += &format!(
                "{:5}.  {:?} ==> {:?}   {:?}\n",
                index, envelope.src, envelope.dest, envelope.body
            )
        });

    let panic_message = |mschedule_path| {
        format!(
            "\nFailed after {test_number} tests\n\n \
                Minimised input ({number_of_shrinks} shrinks):\n\n{}\n \
                History:\n\n{}\n \
                Error message:\n\n  {}\n\n \
                {}\n \
                Seed: {}\n",
            test_case,
            history_string,
            reason,
            match mschedule_path {
                None => "".to_string(),
                Some(path) => format!("Saved schedule: {:?}\n", path),
            },
            seed
        )
    };

    match persist_schedule(Path::new("."), "failure", trace_buffer) {
        Err(err) => {
            warn!("persist_schedule, failed: {}", err);
            panic!("{}", panic_message(None))
        }
        Ok(schedule_path) => panic!("{}", panic_message(Some(schedule_path))),
    };
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

    use crate::simulator::generate::{
        generate_arrival_times, generate_u8, generate_vec, generate_zip_with,
    };

    use super::*;
    use pure_stage::{simulation::SimulationBuilder, StageGraph, Void};

    #[test]
    fn run_stops_when_no_message_to_process_is_left() {
        let mut world: World<EchoMessage> = World::new(Vec::new(), Vec::new());

        let result: &[Envelope<EchoMessage>] = &Vec::new();

        assert_eq!(world.run_world(), Ok(result));
    }

    #[test]
    #[should_panic]
    fn simulate_pure_stage_echo() {
        #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct State(u64, StageRef<Envelope<EchoMessage>, Void>);

        let number_of_tests = 100;
        let seed = 42;
        let number_of_nodes = 1;

        let spawn: fn() -> NodeHandle<EchoMessage> = || {
            let mut network = SimulationBuilder::default();
            let stage = network.stage(
                "echo",
                async |mut state: State, msg: Envelope<EchoMessage>, eff| {
                    if let EchoMessage::Echo { msg_id, echo } = &msg.body {
                        state.0 += 1;
                        // Insert a bug every 5 messages.
                        let echo_response = if state.0.is_multiple_of(5) {
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
                        // println!(" ==> {:?}", reply);
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
        simulate(
            SimulateConfig {
                number_of_tests,
                seed,
                number_of_nodes,
                disable_shrinking: false,
            },
            spawn,
            echo_generator,
            echo_property,
            TraceBuffer::new_shared(0, 0),
            false,
        )
    }

    fn echo_generator(rng: &mut StdRng) -> Vec<Reverse<Entry<EchoMessage>>> {
        let now = Instant::at_offset(Duration::from_secs(0));
        let size = 20;
        let messages = generate_zip_with(
            size,
            generate_vec(generate_u8(0, 128)),
            generate_arrival_times(now, 200.0),
            |msg, arrival_time| {
                Reverse(Entry {
                    arrival_time,
                    envelope: Envelope {
                        src: "c1".to_string(),
                        dest: "n1".to_string(),
                        body: EchoMessage::Echo {
                            msg_id: 0,
                            echo: format!("Please echo {}", msg),
                        },
                    },
                })
            },
        )(rng);
        messages
    }

    // TODO: Take response time into account.
    fn echo_property(history: &History<EchoMessage>) -> Result<(), String> {
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
                    return Err(format!(
                        "No matching response found for echo request: {:?}",
                        msg
                    ));
                }
            }
        }
        Ok(())
    }

    // This shows how we can test external binaries. The test is disabled because building and
    // locating a binary on CI, across all platforms, is annoying.
    #[allow(dead_code)]
    #[ignore]
    fn blackbox_test_echo() {
        let number_of_tests = 100;
        let seed = 42;
        let number_of_nodes = 1;

        let spawn: fn() -> NodeHandle<EchoMessage> = || {
            pipe_node_handle(Path::new("../../target/debug/echo"), &[]).expect("node handle failed")
        };
        simulate(
            SimulateConfig {
                number_of_tests,
                seed,
                number_of_nodes,
                disable_shrinking: false,
            },
            spawn,
            echo_generator,
            echo_property,
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
