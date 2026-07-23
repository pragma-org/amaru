// Copyright 2026 PRAGMA
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

use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Result, bail};

use crate::{emit_rerun_if_exists, write_if_changed};

/// Generate `stake_distribution_<network>_test_cases.rs` in `OUT_DIR`, containing one test
/// case per stake distribution fixture.
pub(crate) fn write_stake_distribution_test_cases_file(network: &str) -> Result<()> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let fixtures_root = manifest_dir.join("tests").join("stake-distributions");
    let network_dir = fixtures_root.join(network);
    let ledger_dir = default_ledger_dir(&manifest_dir, network);

    emit_rerun_if_exists(&network_dir);

    let epochs = stake_distribution_epochs(&network_dir)?;
    let available_epochs = available_ledger_snapshot_epochs(&ledger_dir)?;
    let contents = stake_distribution_test_cases_source(network, &epochs, &available_epochs)?;
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);

    write_if_changed(&out_dir.join(format!("stake_distribution_{network}_test_cases.rs")), &contents)?;

    Ok(())
}

/// Return the local ledger database directory for `network`, at the workspace root.
fn default_ledger_dir(manifest_dir: &Path, network: &str) -> PathBuf {
    manifest_dir.join("../..").join(format!("ledger.{network}.db"))
}

/// List the epochs having a fixture file in `network_dir`, most recent first.
fn stake_distribution_epochs(network_dir: &Path) -> Result<Vec<u64>> {
    if !network_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut epochs = BTreeSet::new();

    for entry in fs::read_dir(network_dir)? {
        let entry = entry?;
        let path = entry.path();

        if !path.is_file() {
            continue;
        }

        if let Some(epoch) = stake_distribution_epoch(&path) {
            epochs.insert(epoch);
        }
    }

    let mut epochs = epochs.into_iter().collect::<Vec<_>>();
    epochs.reverse();
    Ok(epochs)
}

/// Extract the epoch number from an `epoch_<N>.json` or `epoch_<N>.json.zst` fixture file name.
fn stake_distribution_epoch(path: &Path) -> Option<u64> {
    let file_name = path.file_name()?.to_str()?;
    let epoch = file_name.strip_prefix("epoch_")?;
    let epoch = epoch.strip_suffix(".json").or_else(|| epoch.strip_suffix(".json.zst"))?;

    epoch.parse().ok()
}

/// List the epochs for which a ledger snapshot is available locally in `ledger_dir`.
fn available_ledger_snapshot_epochs(ledger_dir: &Path) -> Result<BTreeSet<u64>> {
    emit_rerun_if_exists(ledger_dir);

    if !ledger_dir.is_dir() {
        return Ok(BTreeSet::new());
    }

    let mut epochs = BTreeSet::new();
    for entry in fs::read_dir(ledger_dir)? {
        let entry = entry?;
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }

        if let Some(epoch) = ledger_snapshot_epoch(&path) {
            epochs.insert(epoch);
        }
    }

    Ok(epochs)
}

/// Extract the epoch number from a ledger snapshot directory name.
fn ledger_snapshot_epoch(path: &Path) -> Option<u64> {
    path.file_name()?.to_str()?.parse().ok()
}

/// Split the fixture epochs into those with a local ledger snapshot and those without.
fn partition_fixture_epochs(epochs: &[u64], available_epochs: &BTreeSet<u64>) -> (Vec<u64>, Vec<u64>) {
    epochs.iter().copied().partition(|epoch| available_epochs.contains(epoch))
}

/// Render the test functions comparing stake distributions for `network`: an active test for the
/// epochs with a local ledger snapshot, and an `#[ignore]`d one for the epochs without.
fn stake_distribution_test_cases_source(
    network: &str,
    epochs: &[u64],
    available_epochs: &BTreeSet<u64>,
) -> Result<String> {
    if epochs.is_empty() {
        return Ok(format!(
            r#"// No stake distribution fixtures found for network={network}.
const _: fn(
    amaru_kernel::NetworkName,
    amaru_kernel::Epoch,
) -> Result<(), Box<dyn std::error::Error>> = compare_stake_distribution_with_haskell_node;
"#
        ));
    }

    let network_variant = network_name_to_rust_variant(network)?;
    let (active_epochs, ignored_epochs) = partition_fixture_epochs(epochs, available_epochs);
    let mut contents = String::new();

    contents.push_str(&format!(
        "// Generated from {} fixture epoch(s): {} active, {} ignored.\n",
        epochs.len(),
        active_epochs.len(),
        ignored_epochs.len(),
    ));

    if !active_epochs.is_empty() {
        contents.push_str(&render_stake_distribution_test_function(
            network,
            network_variant,
            "compare",
            &active_epochs,
            false,
        ));
    }

    if !ignored_epochs.is_empty() {
        contents.push_str(&render_stake_distribution_test_function(
            network,
            network_variant,
            "compare_missing_local_snapshot",
            &ignored_epochs,
            true,
        ));
    }

    Ok(contents)
}

/// Render one test function with a `#[test_case]` attribute per epoch.
fn render_stake_distribution_test_function(
    network: &str,
    network_variant: &str,
    prefix: &str,
    epochs: &[u64],
    ignored: bool,
) -> String {
    let mut contents = String::new();

    for epoch in epochs {
        contents.push_str(&format!("#[test_case::test_case({epoch})]\n"));
    }

    if ignored {
        contents.push_str("#[ignore]\n");
    }

    contents.push_str(&format!(
        r#"pub fn {prefix}_{network}_stake_distribution_with_haskell_node(epoch: u64) -> Result<(), Box<dyn std::error::Error>> {{
    compare_stake_distribution_with_haskell_node(
        amaru_kernel::NetworkName::{network_variant},
        amaru_kernel::Epoch::from(epoch),
    )
}}
"#
    ));

    contents
}

/// Map a network name to the corresponding `amaru_kernel::NetworkName` variant.
fn network_name_to_rust_variant(network: &str) -> Result<&'static str> {
    match network {
        "preview" => Ok("Preview"),
        "preprod" => Ok("Preprod"),
        "mainnet" => Ok("Mainnet"),
        _ => bail!("unexpected network name: {network}; expected one of: preview, preprod or mainnet"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stake_distribution_epoch_supports_json_and_json_zst() {
        assert_eq!(stake_distribution_epoch(Path::new("epoch_999.json")), Some(999));
        assert_eq!(stake_distribution_epoch(Path::new("epoch_1000.json.zst")), Some(1000));
        assert_eq!(stake_distribution_epoch(Path::new("generated_test_cases.incl")), None);
    }
}
