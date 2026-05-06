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
    collections::BTreeMap,
    error::Error,
    fs, io,
    path::{Path, PathBuf},
};

use amaru_kernel::NetworkName;
use serde::Deserialize;
use tracing::{info, warn};

use super::repo_root;

const DEFAULT_DB_ANALYSER_IMAGE: &str = "amaru-db-analyser";
const DEFAULT_DB_ANALYSER_REF: &str = "9510ebee3a2319944de4967940e140af0878016e";
const DOCKER_ENV_FILE_NAME: &str = ".env";
const OFFICIAL_CARDANO_NODE_CONFIG_BASE_URL: &str = "https://book.world.dev.cardano.org/environments";

pub(super) struct DbAnalyserBuildConfig {
    pub(super) image: String,
    pub(super) git_ref: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct CardanoNodeConfigManifest {
    #[serde(rename = "AlonzoGenesisFile")]
    alonzo_genesis_file: Option<String>,

    #[serde(rename = "ByronGenesisFile")]
    byron_genesis_file: Option<String>,

    #[serde(rename = "CheckpointsFile")]
    checkpoints_file: Option<String>,

    #[serde(rename = "ConwayGenesisFile")]
    conway_genesis_file: Option<String>,

    #[serde(rename = "ShelleyGenesisFile")]
    shelley_genesis_file: Option<String>,
}

impl CardanoNodeConfigManifest {
    pub(super) fn referenced_files(&self) -> Vec<&str> {
        let mut files = Vec::new();

        for file_name in [
            self.alonzo_genesis_file.as_deref(),
            self.byron_genesis_file.as_deref(),
            self.checkpoints_file.as_deref(),
            self.conway_genesis_file.as_deref(),
            self.shelley_genesis_file.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            files.push(file_name);
        }

        files
    }
}

pub(super) fn resolve_db_analyser_build_config() -> Result<DbAnalyserBuildConfig, Box<dyn Error>> {
    let env_path = docker_env_path();
    let env_values = read_dotenv_values(&env_path)?;
    let image = dotenv_value(&env_values, &["DB_ANALYSER_IMAGE", "AMARU_DB_ANALYSER_IMAGE"])
        .unwrap_or_else(|| DEFAULT_DB_ANALYSER_IMAGE.to_string());
    let git_ref = dotenv_value(&env_values, &["OUROBOROS_CONSENSUS_REF", "DB_ANALYSER_REF", "AMARU_DB_ANALYSER_REF"])
        .unwrap_or_else(|| DEFAULT_DB_ANALYSER_REF.to_string());

    info!(env_file = %env_path.display(), image = %image, git_ref, "using db-analyser build configuration");

    Ok(DbAnalyserBuildConfig { image, git_ref })
}

fn docker_env_path() -> PathBuf {
    repo_root().join("docker").join(DOCKER_ENV_FILE_NAME)
}

fn read_dotenv_values(path: &Path) -> Result<BTreeMap<String, String>, Box<dyn Error>> {
    if !path.is_file() {
        return Ok(BTreeMap::new());
    }

    let mut values = BTreeMap::new();
    for (line_number, raw_line) in fs::read_to_string(path)?.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let line = line.strip_prefix("export ").unwrap_or(line);
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("invalid dotenv entry at {}:{}", path.display(), line_number + 1))?;
        values.insert(key.trim().to_string(), strip_optional_quotes(value.trim()).to_string());
    }

    Ok(values)
}

pub(super) fn dotenv_value(values: &BTreeMap<String, String>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| values.get(*key).cloned())
}

pub(super) fn strip_optional_quotes(value: &str) -> &str {
    if value.len() >= 2 {
        let quoted_with_double = value.starts_with('"') && value.ends_with('"');
        let quoted_with_single = value.starts_with('\'') && value.ends_with('\'');
        if quoted_with_double || quoted_with_single {
            return &value[1..value.len() - 1];
        }
    }

    value
}

pub(super) async fn resolve_config_dir(
    client: &reqwest::Client,
    config_dir: Option<PathBuf>,
    network: NetworkName,
    work_dir: &Path,
) -> Result<PathBuf, Box<dyn Error>> {
    if let Some(config_dir) = config_dir {
        validate_config_dir(&config_dir)?;
        return Ok(config_dir);
    }

    if let Some(config_dir) = bundled_config_dir(network) {
        match validate_config_dir(&config_dir) {
            Ok(()) => {
                info!(config_dir = %config_dir.display(), network = %network, "using bundled cardano-node config");
                return Ok(config_dir);
            }
            Err(error) => {
                warn!(config_dir = %config_dir.display(), error = %error, network = %network, "bundled cardano-node config is incomplete; falling back to official download");
            }
        }
    }

    download_official_config_bundle(client, network, &cached_config_dir(work_dir, network)).await
}

fn bundled_config_dir(network: NetworkName) -> Option<PathBuf> {
    matches!(network, NetworkName::Preprod).then(|| repo_root().join("cardano-node-config"))
}

fn cached_config_dir(work_dir: &Path, network: NetworkName) -> PathBuf {
    work_dir.join("cardano-node-config").join(network.to_string())
}

fn official_config_base_url(network: NetworkName) -> Result<String, Box<dyn Error>> {
    match network {
        NetworkName::Mainnet => Ok(format!("{OFFICIAL_CARDANO_NODE_CONFIG_BASE_URL}/mainnet")),
        NetworkName::Preprod => Ok(format!("{OFFICIAL_CARDANO_NODE_CONFIG_BASE_URL}/preprod")),
        NetworkName::Preview => Ok(format!("{OFFICIAL_CARDANO_NODE_CONFIG_BASE_URL}/preview")),
        NetworkName::Testnet(_) => {
            Err("automatic cardano-node config download is only supported on mainnet, preprod and preview".into())
        }
    }
}

async fn download_official_config_bundle(
    client: &reqwest::Client,
    network: NetworkName,
    config_dir: &Path,
) -> Result<PathBuf, Box<dyn Error>> {
    if validate_config_dir(config_dir).is_ok() {
        info!(config_dir = %config_dir.display(), network = %network, "reusing cached cardano-node config");
        return Ok(config_dir.to_path_buf());
    }

    fs::create_dir_all(config_dir)?;

    let base_url = official_config_base_url(network)?;
    let config_bytes = download_config_file(client, &base_url, "config.json").await?;
    let manifest = serde_json::from_slice::<CardanoNodeConfigManifest>(&config_bytes)?;

    write_file_atomically(&config_dir.join("config.json"), &config_bytes)?;

    for file_name in manifest.referenced_files() {
        let bytes = download_config_file(client, &base_url, file_name).await?;
        write_file_atomically(&config_dir.join(file_name), &bytes)?;
    }

    validate_config_dir(config_dir)?;
    info!(config_dir = %config_dir.display(), network = %network, "downloaded official cardano-node config bundle");

    Ok(config_dir.to_path_buf())
}

async fn download_config_file(
    client: &reqwest::Client,
    base_url: &str,
    file_name: &str,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let response = client.get(format!("{base_url}/{file_name}")).send().await?.error_for_status()?;
    Ok(response.bytes().await?.to_vec())
}

fn validate_config_dir(config_dir: &Path) -> Result<(), Box<dyn Error>> {
    let config_file = config_dir.join("config.json");
    if !config_file.is_file() {
        return Err(format!("missing cardano-node config file at {}", config_file.display()).into());
    }

    let manifest = serde_json::from_slice::<CardanoNodeConfigManifest>(&fs::read(&config_file)?)?;
    for file_name in manifest.referenced_files() {
        let file_path = config_dir.join(file_name);
        if !file_path.is_file() {
            return Err(format!("missing cardano-node config companion file at {}", file_path.display()).into());
        }
    }

    Ok(())
}

fn write_file_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp_path = path.with_extension("tmp");
    fs::write(&tmp_path, bytes)?;
    fs::rename(tmp_path, path)
}
