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
    path::{Path, PathBuf},
    sync::Arc,
};

use amaru_kernel::NetworkName;
use amaru_ledger::store::ReadStore;
use amaru_mithril::{download_from_mithril, from_chunk_for_resume_point, get_latest_chunk};
use amaru_progress_bar::{ProgressBar, TerminalProgressBar};
use amaru_stores::rocksdb::{ReadOnlyRocksDB, RocksDbConfig};
use tracing::info;

pub(super) async fn run(
    network: NetworkName,
    ledger_dir: &Path,
    snapshots_dir: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let target_dir = snapshots_dir.join(network.to_string());
    fs::create_dir_all(&target_dir)?;

    let immutable_dir = target_dir.join("immutable");

    let store = ReadOnlyRocksDB::new(&RocksDbConfig::new(ledger_dir.to_path_buf()))?;
    let tip = store.tip()?;

    let latest_chunk = get_latest_chunk(&immutable_dir)?;
    let from_chunk = from_chunk_for_resume_point(network, latest_chunk, tip)?;

    info!(tip = %tip, from_chunk, "Downloading Mithril immutable chunks");

    download_from_mithril(
        network,
        target_dir,
        from_chunk,
        Arc::new(|length, template| {
            Box::new(TerminalProgressBar::new(length as u64, template)) as Box<dyn ProgressBar + Send + Sync>
        }),
    )
    .await?;

    Ok(immutable_dir)
}
