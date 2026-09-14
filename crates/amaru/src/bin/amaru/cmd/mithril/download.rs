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
use amaru_mithril::download_from_mithril_for_resume_point;
use amaru_progress_bar::{ProgressBar, TerminalProgressBar};
use amaru_stores::rocksdb::{ReadOnlyRocksDB, RocksDbConfig};

pub(super) async fn run(network: NetworkName, ledger_dir: &Path, snapshots_dir: &Path) -> anyhow::Result<PathBuf> {
    let target_dir = snapshots_dir.join(network.to_string());
    fs::create_dir_all(&target_dir)?;

    let store = ReadOnlyRocksDB::new(&RocksDbConfig::new(ledger_dir.to_path_buf()))?;
    let tip = store.tip()?;

    download_from_mithril_for_resume_point(
        network,
        target_dir,
        tip,
        Arc::new(|length, template| {
            Box::new(TerminalProgressBar::new(length as u64, template)) as Box<dyn ProgressBar + Send + Sync>
        }),
    )
    .await
}
