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

use std::{fs::File, io::Write, path::Path, sync::Arc};

use amaru_consensus::headers_tree::data_generation::GeneratedActions;
use parking_lot::Mutex;
use pure_stage::trace_buffer::TraceBuffer;

use crate::simulator::Args;

/// Persist the generated data for a given test run
pub fn persist_generated_data(
    test_run_dir_n: &Path,
    generated_actions: &GeneratedActions,
    persist: bool,
) -> Result<(), anyhow::Error> {
    if !persist {
        return Ok(());
    }
    let path = test_run_dir_n.join("actions.json");
    generated_actions.export_to_file(path.to_str().unwrap());
    Ok(())
}

/// Persist the trace buffer both as cbor (for replay) and json (for animations).
pub fn persist_traces(dir: &Path, trace_buffer: Arc<Mutex<TraceBuffer>>, persist: bool) -> Result<(), anyhow::Error> {
    if !persist {
        return Ok(());
    }

    persist_traces_as_cbor(dir, trace_buffer.clone())?;
    persist_traces_as_json(dir, trace_buffer)?;
    Ok(())
}

/// Persist the traces to a CBOR file
pub fn persist_traces_as_cbor(dir: &Path, trace_buffer: Arc<Mutex<TraceBuffer>>) -> Result<(), anyhow::Error> {
    if trace_buffer.lock().is_empty() {
        return Ok(());
    }

    let messages: Vec<Vec<u8>> = trace_buffer.lock().iter().map(|b| b.to_vec()).collect();
    let path = dir.join("traces.cbor");
    let mut file = File::create(&path)?;
    cbor4ii::serde::to_writer(&mut file, &messages)?;
    Ok(())
}

/// Persist the traces to a JSON file
pub fn persist_traces_as_json(dir: &Path, trace_buffer: Arc<Mutex<TraceBuffer>>) -> Result<(), anyhow::Error> {
    let path = dir.join("traces.json");
    let mut file = File::create(&path)?;

    let traces = trace_buffer.lock().hydrate();
    let traces = traces.iter().map(|trace| trace.1.to_json()).collect::<Vec<_>>();
    file.write_all(serde_json::to_string_pretty(&serde_json::json!(traces))?.as_bytes())?;
    Ok(())
}

/// Persist the seed to .seed file where the filename is the seed value
pub fn persist_args(dir: &Path, args: &Args, persist: bool) -> Result<(), anyhow::Error> {
    if !persist {
        return Ok(());
    }
    let path = dir.join("args.json");
    let mut file = File::create(&path)?;
    let serialized = serde_json::to_string_pretty(&args)?;
    file.write_all(serialized.as_bytes())?;
    Ok(())
}

/// Create a symlink to a directory
pub fn create_symlink_dir(target: &Path, link: &Path) {
    // Clean up existing link or directory first
    if link.exists() {
        std::fs::remove_file(link).or_else(|_| std::fs::remove_dir_all(link)).ok();
    }

    let abs_target = std::fs::canonicalize(target).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink(&abs_target, link).unwrap();
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::symlink_dir;
        symlink_dir(&abs_target, link).unwrap();
    }
}

/// Generate statistics from actions and log them.
pub fn display_actions_statistics(generated_actions: &GeneratedActions) {
    let statistics = generated_actions.statistics();
    tracing::info!(tree_depth=%statistics.tree_depth,
          tree_nodes=%statistics.number_of_nodes,
          tree_forks=%statistics.number_of_fork_nodes,
          "simulate.generate_test_data.statistics");
}
