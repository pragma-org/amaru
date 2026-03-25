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
use amaru_kernel::Transaction;
use parking_lot::Mutex;
use pure_stage::trace_buffer::TraceBuffer;

use crate::simulator::Args;

/// Persist the generated data for a given test run
pub fn persist_generated_data(
    test_run_dir_n: &Path,
    generated_actions: &GeneratedActions,
    transactions: &[Transaction],
    persist: bool,
) -> anyhow::Result<()> {
    if !persist {
        return Ok(());
    }

    persist_actions(test_run_dir_n, generated_actions)?;
    persist_transactions(test_run_dir_n, transactions)?;

    Ok(())
}

fn persist_actions(test_run_dir_n: &Path, generated_actions: &GeneratedActions) -> anyhow::Result<()> {
    let actions_path = test_run_dir_n.join("actions.json");
    generated_actions.export_to_file(actions_path.to_str().ok_or(anyhow::anyhow!("Invalid path {actions_path:?}"))?);
    Ok(())
}

fn persist_transactions(test_run_dir_n: &Path, transactions: &[Transaction]) -> anyhow::Result<()> {
    let transactions_path = test_run_dir_n.join("transactions.json");
    let mut file = File::create(&transactions_path)?;
    let serialized = serde_json::to_string_pretty(transactions)?;
    file.write_all(serialized.as_bytes())?;
    Ok(())
}

/// Persist the trace buffer both as cbor (for replay) and json (for animations).
pub fn persist_traces(dir: &Path, trace_buffer: Arc<Mutex<TraceBuffer>>, persist: bool) -> anyhow::Result<()> {
    if !persist {
        return Ok(());
    }

    persist_traces_as_cbor(dir, trace_buffer.clone())?;
    persist_traces_as_json(dir, trace_buffer)?;
    Ok(())
}

/// Persist the traces to a CBOR file
pub fn persist_traces_as_cbor(dir: &Path, trace_buffer: Arc<Mutex<TraceBuffer>>) -> anyhow::Result<()> {
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
pub fn persist_traces_as_json(dir: &Path, trace_buffer: Arc<Mutex<TraceBuffer>>) -> anyhow::Result<()> {
    let path = dir.join("traces.json");
    let mut file = File::create(&path)?;

    let traces = trace_buffer.lock().hydrate();
    let traces = traces.iter().map(|trace| trace.1.to_json()).collect::<Vec<_>>();
    file.write_all(serde_json::to_string_pretty(&serde_json::json!(traces))?.as_bytes())?;
    Ok(())
}

/// Persist the seed to .seed file where the filename is the seed value
pub fn persist_args(dir: &Path, args: &Args, persist: bool) -> anyhow::Result<()> {
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
    // Clean up existing link or directory first.
    // Use symlink_metadata instead of exists() because exists() follows symlinks
    // and returns false for dangling symlinks, leaving them uncleaned.
    if link.symlink_metadata().is_ok() {
        std::fs::remove_file(link).or_else(|_| std::fs::remove_dir_all(link)).ok();
    }

    let abs_target = std::fs::canonicalize(target).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        // Ignore AlreadyExists errors from concurrent test runs
        match symlink(&abs_target, link) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => panic!("failed to create symlink {link:?} -> {abs_target:?}: {e}"),
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::symlink_dir;
        match symlink_dir(&abs_target, link) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => panic!("failed to create symlink {link:?} -> {abs_target:?}: {e}"),
        }
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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use amaru_kernel::{Hash, TransactionBody, TransactionInput, WitnessSet, size::TRANSACTION_BODY};

    use super::persist_transactions;

    fn create_transactions(number: usize) -> Vec<amaru_kernel::Transaction> {
        (0..number)
            .map(|id| {
                let tx_input = TransactionInput { transaction_id: Hash::new([1; TRANSACTION_BODY]), index: id as u64 };

                let body = TransactionBody::new([tx_input], [], 0);

                amaru_kernel::Transaction {
                    body,
                    witnesses: WitnessSet::default(),
                    is_expected_valid: true,
                    auxiliary_data: None,
                }
            })
            .collect()
    }

    #[test]
    fn persist_transactions_writes_transactions_json() {
        let unique_suffix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("amaru-sim-report-{unique_suffix}"));
        fs::create_dir_all(&temp_dir).unwrap();

        let transactions = create_transactions(2);
        persist_transactions(&temp_dir, &transactions).unwrap();

        let transactions_path = temp_dir.join("transactions.json");
        let written_transactions: Vec<amaru_kernel::Transaction> =
            serde_json::from_str(&fs::read_to_string(&transactions_path).unwrap()).unwrap();

        assert_eq!(written_transactions, transactions);

        fs::remove_dir_all(temp_dir).unwrap();
    }
}
