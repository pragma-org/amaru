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
use crate::simulator::{Args, GeneratedEntries, NodeConfig};
use crate::sync::ChainSyncMessage;
use amaru_consensus::consensus::headers_tree::data_generation::{Action, GeneratedActions};
use amaru_kernel::string_utils::ListToString;
use anyhow::anyhow;
use parking_lot::Mutex;
use pure_stage::trace_buffer::TraceBuffer;
use rand::{SeedableRng, rngs::StdRng};
use serde::Serialize;
use std::fmt::Display;
use std::fs::{File, create_dir_all};
#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(windows)]
use std::os::windows::fs::symlink_dir;
use std::sync::Arc;
use std::time::SystemTime;
use std::{fmt::Debug, io::Write, path::Path};
use tracing::{error, info};

/// Run the simulation
///
/// - Generate a number of messages to be delivered to nodes.
/// - Spawn the nodes, send them messages, collect messages, and check the property.
/// - If there a property fails, shrink the input messages to find a minimal failing case.
/// - Persist the schedule of messages to a file for later replay.
#[allow(clippy::too_many_arguments)]
pub fn simulate<F>(
    simulate_config: &SimulateConfig,
    node_config: &NodeConfig,
    spawn: F,
    generator: impl Fn(Arc<Mutex<StdRng>>) -> GeneratedEntries<ChainSyncMessage, GeneratedActions>,
    property: impl Fn(&History<ChainSyncMessage>, &GeneratedActions) -> Result<(), String>,
    display_test_stats: impl Fn(&GeneratedActions),
    trace_buffer: Arc<Mutex<TraceBuffer>>,
    persist_on_success: bool,
) -> anyhow::Result<()>
where
    F: Fn(String, Arc<Mutex<StdRng>>) -> NodeHandle<ChainSyncMessage>,
{
    let rng = Arc::new(Mutex::new(StdRng::seed_from_u64(simulate_config.seed)));
    info!("Running with seed {}", simulate_config.seed);
    let tests_dir = Path::new("../../target/tests");
    if !tests_dir.exists() {
        create_dir_all(tests_dir)?;
    }

    let now = SystemTime::now();
    let test_run_name = format!("{}", now.duration_since(SystemTime::UNIX_EPOCH)?.as_secs());
    let test_run_dir = tests_dir.join(test_run_name);
    create_dir_all(&test_run_dir)?;
    create_symlink_dir(test_run_dir.as_path(), tests_dir.join("latest").as_path());

    for test_number in 1..=simulate_config.number_of_tests {
        let test_run_dir_n = test_run_dir.join(format!("test-{}", test_number));
        create_dir_all(&test_run_dir_n)?;
        create_symlink_dir(
            test_run_dir_n.as_path(),
            test_run_dir_n.parent().unwrap().join("latest").as_path(),
        );

        info!("");
        info!(
            "Generating test data for test {}/{}",
            test_number, simulate_config.number_of_tests
        );
        let generated_entries = generator(rng.clone());
        let generation_context = generated_entries.generation_context();
        info!("Test data generated, now sending messages");
        display_test_stats(generation_context);
        if persist_on_success {
            persist_generated_entries_as_json(test_run_dir_n.as_path(), &generated_entries)?;
            persist_generated_actions_as_json(
                test_run_dir_n.as_path(),
                &generated_entries.generation_context().actions(),
            )?;
        }

        match run_test(
            simulate_config,
            &spawn,
            &property,
            rng.clone(),
            test_number,
            &generated_entries,
        ) {
            Err(error) => {
                if !persist_on_success {
                    persist_generated_data(&test_run_dir_n, &generated_entries)?;
                }
                persist_traces(test_run_dir.as_path(), trace_buffer.clone())?;
                error!(
                    "Test {}/{} failed! You can inspect the test data in {}",
                    test_number,
                    simulate_config.number_of_tests,
                    test_run_dir.to_str().unwrap()
                );
                return Err(anyhow!(error));
            }
            Ok(()) => {
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
        persist_args(test_run_dir.as_path(), simulate_config, node_config)?;
        persist_traces(test_run_dir.as_path(), trace_buffer.clone())?;
    }
    info!(
        "Success! ({} tests passed)",
        simulate_config.number_of_tests
    );
    display_test_configuration(simulate_config, node_config);
    Ok(())
}

/// Run a single test by spawning nodes, sending them messages and checking the property.
pub fn run_test<Msg, GenerationContext, F>(
    simulate_config: &SimulateConfig,
    spawn: &F,
    property: &impl Fn(&History<Msg>, &GenerationContext) -> Result<(), String>,
    rng: Arc<Mutex<StdRng>>,
    test_number: u32,
    generated_entries: &GeneratedEntries<Msg, GenerationContext>,
) -> Result<(), String>
where
    Msg: Debug + PartialEq + Clone + Serialize + Display,
    F: Fn(String, Arc<Mutex<StdRng>>) -> NodeHandle<Msg>,
{
    let test = test_nodes(
        rng.clone(),
        simulate_config.number_of_nodes,
        &spawn,
        generated_entries.generation_context(),
        &property,
    );

    let entries = generated_entries.entries();
    let result = test(entries);
    match result {
        (history, Err(reason)) => {
            let failure_message = if simulate_config.disable_shrinking {
                let number_of_shrinks = 0;
                create_failure_message(
                    test_number,
                    simulate_config.seed,
                    number_of_shrinks,
                    history,
                    reason,
                )
            } else {
                let (_shrunk_entries, (shrunk_history, result), number_of_shrinks) =
                    shrink(test, entries.clone(), |result| {
                        result.1 == Err(reason.clone())
                    });
                assert_eq!(Err(reason.clone()), result);
                create_failure_message(
                    test_number,
                    simulate_config.seed,
                    number_of_shrinks,
                    shrunk_history,
                    reason,
                )
            };
            Err(failure_message)
        }
        (_history, Ok(())) => Ok(()),
    }
}

/// Spawn a given number of nodes, run the simulation and check the property.
fn test_nodes<Msg, GenerationContext, F>(
    rng: Arc<Mutex<StdRng>>,
    number_of_nodes: u8,
    spawn: F,
    generation_context: &GenerationContext,
    property: impl Fn(&History<Msg>, &GenerationContext) -> Result<(), String>,
) -> impl Fn(&[Entry<Msg>]) -> (History<Msg>, Result<(), String>)
where
    Msg: Debug + PartialEq + Clone + Display,
    F: Fn(String, Arc<Mutex<StdRng>>) -> NodeHandle<Msg>,
{
    move |entries| {
        let rng_clone = rng.clone();
        let node_handles: Vec<_> = (1..=number_of_nodes)
            .map(|i| {
                let node_id = format!("n{}", i);
                (node_id.clone(), spawn(node_id, rng_clone.clone()))
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
    number_of_shrinks: u32,
    history: History<Msg>,
    reason: String,
) -> String {
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

    format!(
        "\nFailed after {test_number} tests\n\n \
                Minimised input ({number_of_shrinks} shrinks):\n\n \
                History:\n\n{}\n \
                Error message:\n  {}\n\n \
                Seed: {}\n",
        history_string, reason, seed
    )
}

fn persist_generated_data(
    test_run_dir_n: &Path,
    generated_entries: &GeneratedEntries<ChainSyncMessage, GeneratedActions>,
) -> Result<(), anyhow::Error> {
    persist_generated_entries_as_json(test_run_dir_n, generated_entries)?;
    persist_generated_actions_as_json(
        test_run_dir_n,
        &generated_entries.generation_context().actions(),
    )?;
    Ok(())
}

fn persist_traces(dir: &Path, trace_buffer: Arc<Mutex<TraceBuffer>>) -> Result<(), anyhow::Error> {
    persist_traces_as_cbor(dir, trace_buffer.clone())?;
    persist_traces_as_json(dir, trace_buffer.clone())?;
    Ok(())
}

fn persist_traces_as_cbor(
    dir: &Path,
    trace_buffer: Arc<Mutex<TraceBuffer>>,
) -> Result<(), anyhow::Error> {
    if trace_buffer.lock().is_empty() {
        return Ok(());
    }

    let messages: Vec<Vec<u8>> = trace_buffer.lock().iter().map(|b| b.to_vec()).collect();
    let path = dir.join("traces.cbor");
    let mut file = File::create(&path)?;
    cbor4ii::serde::to_writer(&mut file, &messages)?;
    Ok(())
}

/// Persist the seed to .seed file where the filename is the seed value
fn persist_args(
    dir: &Path,
    simulate_config: &SimulateConfig,
    node_config: &NodeConfig,
) -> Result<(), anyhow::Error> {
    let args = Args::from_configs(simulate_config, node_config);
    let path = dir.join("args.json");
    let mut file = File::create(&path)?;
    let serialized = serde_json::to_string_pretty(&args)?;
    file.write_all(serialized.as_bytes())?;
    Ok(())
}

/// Persist the traces to a JSON file
fn persist_traces_as_json(
    dir: &Path,
    trace_buffer: Arc<Mutex<TraceBuffer>>,
) -> Result<(), anyhow::Error> {
    let path = dir.join("traces.json");
    let mut file = File::create(&path)?;

    let traces = trace_buffer.lock().hydrate();
    let traces = traces
        .iter()
        .map(|trace| trace.to_json())
        .collect::<Vec<_>>();
    file.write_all(serde_json::to_string_pretty(&serde_json::json!(traces))?.as_bytes())?;
    Ok(())
}

/// Persist the generated entries to a JSON file
fn persist_generated_entries_as_json(
    dir: &Path,
    generated_entries: &GeneratedEntries<ChainSyncMessage, GeneratedActions>,
) -> Result<(), anyhow::Error> {
    let path = dir.join("entries.json");
    generated_entries.export_to_file(path.to_str().unwrap());
    Ok(())
}

/// Persist the generated actions to a JSON file
fn persist_generated_actions_as_json(dir: &Path, actions: &[Action]) -> Result<(), anyhow::Error> {
    let path = dir.join("actions.json");
    let all_lines: Vec<String> = actions.iter().map(|action| action.pretty_print()).collect();
    let mut file = File::create(&path)?;
    write!(file, "{}", all_lines.list_to_string(",\n"))?;
    Ok(())
}

/// Create a symlink to a directory
fn create_symlink_dir(target: &Path, link: &Path) {
    // Clean up existing link or directory first
    if link.exists() {
        std::fs::remove_file(link)
            .or_else(|_| std::fs::remove_dir_all(link))
            .ok();
    }

    let abs_target = std::fs::canonicalize(target).unwrap();
    #[cfg(unix)]
    {
        symlink(&abs_target, link).unwrap();
    }
    #[cfg(windows)]
    {
        symlink_dir(&abs_target, link).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::echo::{EchoMessage, Envelope, echo_generator, echo_property, spawn_echo_node};

    #[test]
    fn run_stops_when_no_message_to_process_is_left() {
        let mut world: World<EchoMessage> = World::new(vec![], vec![]);
        assert_eq!(world.run_world(), Ok(&[] as &[Envelope<EchoMessage>]));
    }

    #[test]
    fn simulate_pure_stage_echo() {
        let simulate_config = SimulateConfig::default()
            .with_number_of_tests(10)
            .with_seed(42)
            .with_number_of_nodes(1)
            .disable_shrinking();

        let rng = Arc::new(Mutex::new(StdRng::seed_from_u64(simulate_config.seed)));
        let generated_entries = echo_generator(rng.clone());

        let failure = run_test(
            &simulate_config,
            &spawn_echo_node,
            &echo_property,
            rng,
            1,
            &generated_entries,
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

        let spawn: fn(String, Arc<Mutex<StdRng>>) -> NodeHandle<EchoMessage> = |_node_id, _rng| {
            NodeHandle::from_executable(Path::new("../../target/debug/echo"), &[])
                .expect("node handle failed")
        };

        let rng = Arc::new(Mutex::new(StdRng::seed_from_u64(simulate_config.seed)));
        let generated_entries = echo_generator(rng.clone());

        let failure_message = run_test(
            &simulate_config,
            &spawn,
            &echo_property,
            rng,
            1,
            &generated_entries,
        )
        .err();
        assert!(failure_message.is_some());
    }
}
