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

use crate::simulator::shrink::shrink;
use crate::simulator::simulate_config::SimulateConfig;
use crate::simulator::world::{Entry, NodeHandle};
pub(crate) use crate::simulator::world::{History, World};
use crate::simulator::{GeneratedEntries, NodeConfig};
use parking_lot::Mutex;
use pure_stage::trace_buffer::TraceBuffer;
use rand::{SeedableRng, rngs::StdRng};
use serde::Serialize;
use std::fmt::Display;
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use std::{fmt::Debug, io::Write, path::Path};
use tracing::{info, warn};

/// Run the simulation
///
/// - Generate a number of messages to be delivered to nodes.
/// - Spawn the nodes, send them messages, collect messages, and check the property.
/// - If there a property fails, shrink the input messages to find a minimal failing case.
/// - Persist the schedule of messages to a file for later replay.
#[allow(clippy::too_many_arguments)]
pub fn simulate<Msg, GenerationContext, F>(
    simulate_config: &SimulateConfig,
    node_config: &NodeConfig,
    spawn: F,
    generator: impl Fn(&mut StdRng) -> GeneratedEntries<Msg, GenerationContext>,
    property: impl Fn(&History<Msg>, &GenerationContext) -> Result<(), String>,
    display_test_stats: impl Fn(&GenerationContext),
    trace_buffer: Arc<Mutex<TraceBuffer>>,
    persist_on_success: bool,
) -> Result<(), String>
where
    Msg: Debug + PartialEq + Clone + Serialize + Display,
    F: Fn(String) -> NodeHandle<Msg>,
{
    let mut rng = StdRng::seed_from_u64(simulate_config.seed);

    for test_number in 1..=simulate_config.number_of_tests {
        info!("");
        info!(
            "Generating test data for test {}/{}",
            test_number, simulate_config.number_of_tests
        );
        let generated_entries = generator(&mut rng);
        let entries = generated_entries.entries();
        let generation_context = generated_entries.generation_context();
        info!("Test data generated, now sending messages");
        display_test_stats(generation_context);

        let test = test_nodes(
            simulate_config.number_of_nodes,
            &spawn,
            generation_context,
            &property,
        );

        let result = test(entries);
        match result {
            (history, Err(reason)) => {
                let failure_message = if simulate_config.disable_shrinking {
                    let number_of_shrinks = 0;
                    create_failure_message(
                        test_number,
                        simulate_config.seed,
                        entries,
                        number_of_shrinks,
                        history,
                        trace_buffer.clone(),
                        reason,
                    )
                } else {
                    let (shrunk_entries, (shrunk_history, result), number_of_shrinks) =
                        shrink(test, entries.clone(), |result| {
                            result.1 == Err(reason.clone())
                        });
                    assert_eq!(Err(reason.clone()), result);
                    create_failure_message(
                        test_number,
                        simulate_config.seed,
                        &shrunk_entries,
                        number_of_shrinks,
                        shrunk_history,
                        trace_buffer.clone(),
                        reason,
                    )
                };
                return Err(failure_message);
            }
            (_history, Ok(())) => {
                display_test_stats(generation_context);
                info!(
                    "Test {test_number}/{} succeeded!",
                    simulate_config.number_of_tests
                );
                info!("");
            }
        }
    }
    if persist_on_success {
        persist_schedule(Path::new("."), "success", trace_buffer)
    }
    info!(
        "Success! ({} tests passed)",
        simulate_config.number_of_tests
    );
    display_test_configuration(simulate_config, node_config);
    Ok(())
}

/// Spawn a given number of nodes, run the simulation and check the property.
fn test_nodes<Msg, GenerationContext, F>(
    number_of_nodes: u8,
    spawn: F,
    generation_context: &GenerationContext,
    property: impl Fn(&History<Msg>, &GenerationContext) -> Result<(), String>,
) -> impl Fn(&[Entry<Msg>]) -> (History<Msg>, Result<(), String>)
where
    Msg: Debug + PartialEq + Clone + Display,
    F: Fn(String) -> NodeHandle<Msg>,
{
    move |entries| {
        let node_handles: Vec<_> = (1..=number_of_nodes)
            .map(|i| {
                let node_id = format!("n{}", i);
                (node_id.clone(), spawn(node_id))
            })
            .collect();

        let mut world = World::new(entries.to_vec(), node_handles);

        match world.run_world() {
            Ok(history) => {
                let history = History(history.to_vec());
                let result = property(&history, generation_context);
                (history, result)
            }
            Err((reason, history)) => (History(history.to_vec()), Err(reason)),
        }
    }
}

fn display_test_configuration(simulate_config: &SimulateConfig, node_config: &NodeConfig) {
    info!("Number of tests: {}", simulate_config.number_of_tests);
    info!(
        "Number of upstream peers: {}",
        node_config.number_of_upstream_peers
    );
}

/// Create a detailed failure message including the test number, seed, shrunk entries,
/// number of shrinks, history and reason for failure.
fn create_failure_message<Msg: Debug>(
    test_number: u32,
    seed: u64,
    entries: &[Entry<Msg>],
    number_of_shrinks: u32,
    history: History<Msg>,
    trace_buffer: Arc<parking_lot::Mutex<TraceBuffer>>,
    reason: String,
) -> String {
    let mut test_case = String::new();
    entries
        .iter()
        .for_each(|entry| test_case += &format!("  {:?}\n", entry.envelope));
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

    let failure_message = |mschedule_path| {
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

    match persist_schedule_to(Path::new("."), "failure", trace_buffer) {
        Err(err) => {
            warn!("persist_schedule, failed: {}", err);
            failure_message(None)
        }
        Ok(schedule_path) => failure_message(Some(schedule_path)),
    }
}

/// Persist the schedule to a file, logging success or failure to stderr.
fn persist_schedule(dir: &Path, prefix: &str, trace_buffer: Arc<Mutex<TraceBuffer>>) {
    match persist_schedule_to(dir, prefix, trace_buffer) {
        Err(err) => eprintln!("{}", err),
        Ok(path) => eprintln!("Saved schedule: {:?}", path),
    }
}

/// Persist the schedule to a file, returning the path to the file or an error.
fn persist_schedule_to(
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
        now.duration_since(SystemTime::UNIX_EPOCH)?.as_secs()
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
    use super::*;
    use crate::echo::{EchoMessage, Envelope, echo_generator, echo_property, spawn_echo_node};
    use pure_stage::Instant;
    use std::fs;
    use std::time::Duration;

    #[test]
    fn run_stops_when_no_message_to_process_is_left() {
        let mut world: World<EchoMessage> = World::new(vec![], vec![]);
        assert_eq!(world.run_world(), Ok(&[] as &[Envelope<EchoMessage>]));
    }

    #[test]
    fn simulate_pure_stage_echo() {
        let simulate_config = SimulateConfig::default()
            .with_number_of_tests(100)
            .with_seed(42)
            .with_number_of_nodes(1)
            .disable_shrinking();
        let node_config = NodeConfig::default();

        let failure = simulate(
            &simulate_config,
            &node_config,
            spawn_echo_node,
            echo_generator,
            echo_property,
            |_| (),
            TraceBuffer::new_shared(0, 0),
            false,
        )
        .err();
        assert!(failure.is_some());
    }

    // This shows how we can test external binaries. The test is disabled because building and
    // locating a binary on CI, across all platforms, is annoying.
    #[expect(dead_code)]
    #[ignore]
    fn blackbox_test_echo() {
        let simulate_config = SimulateConfig::default()
            .with_number_of_tests(100)
            .with_seed(42)
            .with_number_of_nodes(1)
            .disable_shrinking();
        let node_config = NodeConfig::default();

        let spawn: fn(String) -> NodeHandle<EchoMessage> = |_node_id| {
            NodeHandle::from_executable(Path::new("../../target/debug/echo"), &[])
                .expect("node handle failed")
        };
        let failure_message = simulate(
            &simulate_config,
            &node_config,
            spawn,
            echo_generator,
            echo_property,
            |_| (),
            TraceBuffer::new_shared(0, 0),
            false,
        )
        .err();
        assert!(failure_message.is_some());
    }

    #[test]
    fn persist_empty_schedule() {
        let schedule = TraceBuffer::new_shared(0, 0);
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().to_path_buf();
        let result = persist_schedule_to(&path, "test", schedule);
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

        let result = persist_schedule_to(&path, "test", schedule);
        assert!(result.is_ok(), "{:?}", result);

        let file_size = fs::metadata(result.unwrap()).unwrap().len();
        assert_eq!(file_size, 63)
    }
}
