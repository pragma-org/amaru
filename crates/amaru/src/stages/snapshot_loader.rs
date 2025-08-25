// Copyright 2024 PRAGMA
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

use amaru_kernel::{Point, network::NetworkName};
use amaru_ledger::store::{Snapshot, Store};
use amaru_stores::in_memory::MemoryStore;
use std::{
    error::Error,
    path::{Path, PathBuf},
};

pub fn load_snapshots_into_store(
    dir: &Path,
    store: &mut MemoryStore,
    network: NetworkName,
) -> Result<(), Box<dyn Error>> {
    // Collect and sort snapshot files
    let mut entries: Vec<(PathBuf, u64)> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("cbor"))
        .map(|path| {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| format!("invalid snapshot filename {:?}", path))?;
            let slot = stem
                .split('.')
                .next()
                .ok_or_else(|| format!("missing slot prefix in {:?}", path))?
                .parse::<u64>()
                .map_err(|e| format!("invalid slot in {:?}: {}", path, e))?;
            Ok((path, slot))
        })
        .collect::<Result<_, Box<dyn Error>>>()?;

    if entries.len() < 3 {
        return Err(format!(
            "expected at least 3 snapshot files in {:?}, found {}",
            dir,
            entries.len()
        )
        .into());
    }
    entries.sort_by_key(|(_, slot)| *slot);

    // Load snapshot files into MemoryStore
    for (path, _) in entries {
        let bytes = std::fs::read(&path)?;
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("invalid snapshot filename {:?}", path))?;
        let point = Point::try_from(stem)?;

        store.apply_snapshot_bytes(
            &bytes,
            &point,
            network,
            &amaru_progress_bar::new_terminal_progress_bar,
        )?;

        let epoch = store.epoch();
        store.next_snapshot(epoch)?;
    }

    Ok(())
}
