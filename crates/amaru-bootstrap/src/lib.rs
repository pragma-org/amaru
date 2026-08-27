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

//! Cold-start bootstrap for Amaru: download epoch snapshots and import them into
//! empty ledger and chain databases so a node can start and sync.
//!
//! This crate is **not** required for steady-state operation. Once the stores
//! exist, depend only on `amaru-node` and start the node with `NodeBuilder`
//! (or `build_and_run_node`). See EDR 031 (Embedding Amaru).
//!
//! # What the product binary does
//!
//! The operator entry point is `amaru node bootstrap` (implemented in the
//! `amaru` binary). That command is a thin wrapper around this crate:
//!
//! 1. Resolve network, global parameters, and store paths
//!    (`ledger.<network>.db`, `chain.<network>.db` by default).
//! 2. **Refuse to run** if either directory already contains files — bootstrap
//!    always starts from empty dirs (remove them manually, e.g. with
//!    `amaru node rm`, if you want a clean start).
//! 3. Call [`bootstrap()`] with store paths, the snapshot cache directory
//!    ([`default_snapshots_dir`]), an optional target epoch, and an [`S3Config`]
//!    (defaults point at the public Amaru snapshot CDN).
//!
//! Equivalent library usage:
//!
//! ```ignore
//! use std::path::PathBuf;
//!
//! use amaru_bootstrap::{S3Config, bootstrap, default_snapshots_dir};
//! use amaru_kernel::NetworkName;
//! use amaru_node::{default_chain_dir, default_ledger_dir};
//!
//! # async fn example() -> anyhow::Result<()> {
//! let network = NetworkName::Preprod;
//! let global = network.as_global_parameters().cloned().expect("known network");
//! let ledger_dir = PathBuf::from(default_ledger_dir(network));
//! let chain_dir = PathBuf::from(default_chain_dir(network));
//! let snapshots_dir = PathBuf::from(default_snapshots_dir(network));
//!
//! // Caller must ensure ledger_dir and chain_dir are empty or missing.
//! bootstrap(
//!     network,
//!     &global,
//!     ledger_dir,
//!     chain_dir,
//!     snapshots_dir,          // `snapshots/<network>/`; reuses archives already on disk
//!     /* target_epoch */ None, // latest window available on the CDN
//!     S3Config::default(),     // or override bucket / public_url
//! )
//! .await?;
//!
//! // Then: NodeBuilder::new(network)?.ledger_dir(...).chain_dir(...).build_and_run(rt.handle())?
//! # Ok(())
//! # }
//! ```
//!
//! CLI flags map to the same inputs:
//!
//! | CLI (`amaru node bootstrap`) | Library |
//! |------------------------------|---------|
//! | `--network` | `NetworkName` |
//! | `--epoch` | `target_epoch: Option<Epoch>` — epoch Amaru should **start from**; needs three prior consecutive snapshots |
//! | `--ledger-dir` / `--chain-dir` | `ledger_dir` / `chain_dir` |
//! | (CWD) `snapshots/<network>/` | `snapshots_dir` — [`default_snapshots_dir`] |
//! | `--s3-bucket`, `--s3-endpoint`, `--s3-region`, `--s3-public-url` | [`S3Config`] |
//!
//! # What [`bootstrap()`] does
//!
//! Amaru needs **three consecutive epoch snapshots** ending at the epoch before
//! the start epoch (see `docs/BOOTSTRAP.md`). The function:
//!
//! 1. Lists published snapshots via the public CDN ([`AnonymousS3Client`] /
//!    [`S3Config::public_url`]), using `<network>/index.json`.
//! 2. Selects the required three-epoch window (latest available, or the window
//!    that allows starting at `target_epoch` when set).
//! 3. Downloads `.tar.zst` archives into `snapshots_dir` (CLI: [`default_snapshots_dir`],
//!    `snapshots/<network>/`), reusing files already on disk when present.
//! 4. Imports the three snapshots into the **ledger** RocksDB directory in order.
//! 5. Creates the **chain** store, seeds chain state from the newest snapshot,
//!    and imports packaged bootstrap blocks so consensus can attach at the tip.
//!
//! After success, `amaru node run` (or an embedder’s `NodeBuilder`) can open
//! the same directories and follow the network.
//!
//! # Other entry points (also used by the `amaru` CLI)
//!
//! Not every bootstrap-related task goes through [`bootstrap()`]:
//!
//! - **[`import_snapshots`] / [`import_snapshots_from_directory`]** — import
//!   already-downloaded snapshot archives into a ledger dir only
//!   (`amaru dev ledger states import`, convert helpers). No CDN fetch.
//! - **[`validate_publishable_snapshot_archive`]** — check an archive before
//!   upload (`amaru snapshot create` / `publish`).
//! - **[`fetch_headers_from_points`]** — pull headers from a live peer
//!   (`amaru dev chain fetch`).
//! - **Authenticated upload** — [`S3Client`] with credentials for
//!   `amaru snapshot publish` / reindex; cold-start download uses
//!   [`AnonymousS3Client`] only.
//!
//! # Modules
//!
//! - [`mod@aws`] — S3/CDN config and clients (anonymous download + authenticated upload).
//! - [`mod@bootstrap`] — selection, download, import, and chain-store seeding.
//! - [`mod@cardano_node`] — parsers for Haskell-node / db-analyser snapshot formats
//!   used during import.
//!
//! # Tokio
//!
//! Network and filesystem work is async. Call from a Tokio runtime (the product
//! binary uses `RuntimeKind::Io` for bootstrap). The crate does not install a
//! runtime for you.

pub mod aws;
pub mod bootstrap;
pub mod cardano_node;
mod progress;

pub use aws::{
    ARCHIVE_EXTENSION, AnonymousS3Client, DEFAULT_BUCKET, DEFAULT_ENDPOINT, DEFAULT_PUBLIC_URL, DEFAULT_REGION,
    S3Client, S3Config, S3Snapshot,
};
pub use bootstrap::{
    BOOTSTRAP_HEADERS_PER_POINT, BootstrapError, ChainState, ImportError, InitialNonces, bootstrap,
    fetch_headers_from_points, import_headers, import_packaged_blocks, import_snapshots,
    import_snapshots_from_directory, store_chain_state, validate_publishable_snapshot_archive,
};

/// Default on-disk directory for downloaded bootstrap archives.
pub const SNAPSHOTS_PATH: &str = "snapshots";

pub fn default_snapshots_dir(network: amaru_kernel::NetworkName) -> String {
    format!("{}/{}", SNAPSHOTS_PATH, network.to_string().to_lowercase())
}
