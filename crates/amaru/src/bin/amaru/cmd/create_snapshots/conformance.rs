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
    fs,
    io::{self, Cursor},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use amaru_kernel::NetworkName;
use flate2::{Compression, GzBuilder};
use tar::{Builder, Header};
use tracing::info;

use super::EpochTarget;

/// Mapping from extractor output directory name to repo (archive) directory name.
/// The extractor uses camelCase names; the repo uses kebab-case.
const DATASETS: &[(&str, &str)] = &[
    ("pots", "pots"),
    ("nonces", "nonces"),
    ("pools", "pools"),
    ("dreps", "dreps"),
    ("rewardsProvenance", "rewards-provenance"),
];

pub(super) fn conformance_archive_path(snapshot_output_dir: &Path, network: NetworkName, epoch: u64) -> PathBuf {
    snapshot_output_dir.join(format!("conformance-{network}-{epoch}.tar.gz"))
}

pub(super) fn existing_conformance_archive_paths(
    snapshot_output_dir: &Path,
    network: NetworkName,
    targets: &[EpochTarget],
) -> Vec<PathBuf> {
    targets
        .iter()
        .map(|t| conformance_archive_path(snapshot_output_dir, network, u64::from(t.epoch)))
        .filter(|p| p.is_file())
        .collect()
}

pub(super) fn ensure_haskell_extractor_binary() -> Result<String, Box<dyn std::error::Error>> {
    let binary = "haskell-node-extractor";

    let status = Command::new(binary).arg("--help").stdout(Stdio::null()).stderr(Stdio::null()).status();

    match status {
        Ok(_) => {
            info!(binary, "using haskell-node-extractor binary from $PATH");
            Ok(binary.to_owned())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err("haskell-node-extractor was not found in $PATH. \
             Build it from conformance-tests/haskell-node-extractor and add it to your $PATH."
            .into()),
        Err(error) => Err(format!(
            "failed to execute haskell-node-extractor preflight: {}. Ensure the binary is executable.",
            error
        )
        .into()),
    }
}

pub(super) fn run_haskell_extractor(
    binary: &str,
    snapshot_dir: &Path,
    output_dir: &Path,
    network: NetworkName,
) -> Result<(), Box<dyn std::error::Error>> {
    let snapshot_state = snapshot_dir.join("state");
    let network_flag = format!("--{network}");

    info!(
        binary,
        snapshot = %snapshot_state.display(),
        output = %output_dir.display(),
        network = %network,
        "running haskell-node-extractor",
    );

    let result = Command::new(binary)
        .current_dir(output_dir)
        .args(["extract", &network_flag, "--snapshot"])
        .arg(&snapshot_state)
        .status()?;

    if !result.success() {
        return Err(format!("haskell-node-extractor failed for snapshot '{}'", snapshot_state.display()).into());
    }

    Ok(())
}

pub(super) fn package_conformance_archive(
    output_dir: &Path,
    archive_path: &Path,
    epoch: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp_path = archive_path.with_extension("tmp");
    let bytes = build_conformance_archive_bytes(output_dir, epoch)?;
    fs::write(&tmp_path, bytes)?;
    fs::rename(tmp_path, archive_path)?;
    Ok(())
}

fn build_conformance_archive_bytes(output_dir: &Path, epoch: u64) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let encoder = GzBuilder::new().mtime(0).write(Vec::new(), Compression::default());
    let mut tar = Builder::new(encoder);

    for (extractor_name, archive_name) in DATASETS {
        let file_name = format!("{epoch}.json");
        let file_path = output_dir.join("data").join(extractor_name).join(&file_name);
        if !file_path.is_file() {
            return Err(format!("conformance file not found: {}", file_path.display()).into());
        }
        let archive_path = PathBuf::from(archive_name).join(&file_name);
        append_directory_entry(&mut tar, Path::new(archive_name))?;
        append_file_entry(&mut tar, &archive_path, &file_path)?;
    }

    let encoder = tar.into_inner()?;
    Ok(encoder.finish()?)
}

fn append_directory_entry<W: io::Write>(tar: &mut Builder<W>, archive_path: &Path) -> io::Result<()> {
    let mut header = Header::new_gnu();
    header.set_entry_type(tar::EntryType::Directory);
    header.set_mode(0o755);
    header.set_size(0);
    header.set_mtime(0);
    header.set_uid(0);
    header.set_gid(0);
    header.set_cksum();
    tar.append_data(&mut header, archive_path, io::empty())
}

fn append_file_entry<W: io::Write>(tar: &mut Builder<W>, archive_path: &Path, file_path: &Path) -> io::Result<()> {
    let bytes = fs::read(file_path)?;
    let mut header = Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_mode(0o644);
    header.set_size(bytes.len() as u64);
    header.set_mtime(0);
    header.set_uid(0);
    header.set_gid(0);
    header.set_cksum();
    tar.append_data(&mut header, archive_path, Cursor::new(bytes))
}
