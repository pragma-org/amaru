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

use amaru_kernel::network::NetworkName;
use include_dir::{Dir, include_dir};
use std::{error::Error, path::PathBuf};

pub mod bootstrap;
pub mod exit;
pub mod observability;
pub mod point;
pub mod stages;

const SNAPSHOTS_PATH: &str = "snapshots";
const BOOTSTRAP_PATH: &str = "crates/amaru/config/bootstrap";
static BOOTSTRAP_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/config/bootstrap");

pub fn default_ledger_dir(network: NetworkName) -> String {
    format!("./ledger.{}.db", network.to_string().to_lowercase())
}

pub fn default_chain_dir(network: NetworkName) -> String {
    format!("./chain.{}.db", network.to_string().to_lowercase())
}

pub fn bootstrap_config_dir(network: NetworkName) -> PathBuf {
    format!("{}/{}", BOOTSTRAP_PATH, network.to_string().to_lowercase()).into()
}

pub fn default_snapshots_dir(network: NetworkName) -> String {
    format!("{}/{}", SNAPSHOTS_PATH, network)
}

pub fn get_bootstrap_file(
    network: NetworkName,
    name: &str,
) -> Result<Option<Vec<u8>>, Box<dyn Error>> {
    let path = format!("{}/{}", network.to_string().to_lowercase(), name);
    Ok(BOOTSTRAP_DIR.get_file(path).map(|f| f.contents().into()))
}

pub fn get_bootstrap_headers(
    network: NetworkName,
) -> Result<impl Iterator<Item = Vec<u8>>, Box<dyn Error>> {
    let path = format!("{}/headers/*", network.to_string().to_lowercase());
    Ok(BOOTSTRAP_DIR
        .find(&path)?
        .filter_map(|f| f.as_file())
        .map(|f| f.contents().into()))
}
