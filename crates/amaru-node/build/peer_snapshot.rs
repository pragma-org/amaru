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

//! Best-effort download of Cardano peer snapshots for networks listed in
//! [`amaru_kernel::PEER_SNAPSHOT_NETWORKS`] (for example mainnet, preprod, preview).
//!
//! Snapshots are staged under `config/peer-snapshots/{network}/peer-snapshot.json`
//! (not committed). When GitHub is reachable, files are refreshed from
//! `cardano-foundation/cardano-configurations` at the youngest commit at or
//! before the Amaru HEAD committer timestamp.
//!
//! Set `AMARU_PEER_SNAPSHOT_REQUIRED=1` to fail the build if any known network is
//! still missing after the fetch attempt (used by release CI). Optional
//! `GITHUB_TOKEN` / `GH_TOKEN` raises GitHub API rate limits.

use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use amaru_kernel::PEER_SNAPSHOT_NETWORKS;
use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::{emit_rerun_if_exists, write_if_changed};

const CONFIGS_REPO: &str = "cardano-foundation/cardano-configurations";
const USER_AGENT: &str = "amaru-build (https://github.com/pragma-org/amaru)";
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

fn peer_snapshot_network_names() -> impl Iterator<Item = &'static str> {
    PEER_SNAPSHOT_NETWORKS.iter().map(|n| n.as_str())
}

/// Stage peer snapshots, embed available ones into `OUT_DIR`, and optionally
/// fail when required files are missing.
pub fn prepare_peer_snapshots() -> Result<()> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").context("CARGO_MANIFEST_DIR")?);
    let out_dir = PathBuf::from(env::var("OUT_DIR").context("OUT_DIR")?);
    let staging_root = manifest_dir.join("config").join("peer-snapshots");

    println!("cargo:rerun-if-env-changed=AMARU_PEER_SNAPSHOT_REQUIRED");
    println!("cargo:rerun-if-env-changed=AMARU_SKIP_PEER_SNAPSHOT_FETCH");
    println!("cargo:rerun-if-env-changed=GITHUB_TOKEN");
    println!("cargo:rerun-if-env-changed=GH_TOKEN");

    for network in peer_snapshot_network_names() {
        emit_rerun_if_exists(&staging_path(&staging_root, network));
    }
    emit_rerun_if_exists(&configs_commit_path(&staging_root));

    let required = env_flag("AMARU_PEER_SNAPSHOT_REQUIRED");
    let skip_fetch = env_flag("AMARU_SKIP_PEER_SNAPSHOT_FETCH");

    let mut configs_sha: Option<String> = read_staged_configs_commit(&staging_root);
    if !skip_fetch {
        match fetch_all(&staging_root) {
            Ok(sha) => {
                configs_sha = Some(sha.clone());
                write_staged_configs_commit(&staging_root, &sha)?;
            }
            Err(err) => {
                println!("cargo:warning=peer snapshot fetch failed (continuing with staged files if any): {err:#}");
            }
        }
    } else {
        println!("cargo:warning=peer snapshot fetch skipped (AMARU_SKIP_PEER_SNAPSHOT_FETCH)");
    }

    let mut present = Vec::new();
    let mut missing = Vec::new();
    for network in peer_snapshot_network_names() {
        let path = staging_path(&staging_root, network);
        if path.is_file() {
            let dest = out_dir.join(format!("peer_snapshot_{network}.json"));
            fs::copy(&path, &dest).with_context(|| format!("copy {} -> {}", path.display(), dest.display()))?;
            present.push(network);
        } else {
            println!(
                "cargo:warning=peer snapshot missing for {network}; place a file at {} or allow network fetch",
                path.display()
            );
            missing.push(network);
        }
    }

    if required && !missing.is_empty() {
        bail!("AMARU_PEER_SNAPSHOT_REQUIRED=1 but peer snapshots missing for: {}", missing.join(", "));
    }

    write_embed_module(&out_dir, &present, configs_sha.as_deref())?;
    Ok(())
}

fn fetch_all(staging_root: &Path) -> Result<String> {
    let amaru_time = amaru_head_committer_date().context("determine Amaru HEAD committer date")?;
    let agent = http_agent()?;
    let sha = resolve_configs_commit(&agent, &amaru_time)
        .with_context(|| format!("resolve {CONFIGS_REPO} commit at or before {amaru_time}"))?;

    for network in peer_snapshot_network_names() {
        match download_snapshot(&agent, &sha, network) {
            Ok(bytes) => {
                let path = staging_path(staging_root, network);
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                let tmp = path.with_extension("json.tmp");
                fs::write(&tmp, &bytes)?;
                fs::rename(&tmp, &path)?;
            }
            Err(err) => {
                println!(
                    "cargo:warning=failed to download peer snapshot for {network} at {CONFIGS_REPO}@{sha}: {err:#}"
                );
            }
        }
    }

    Ok(sha)
}

fn amaru_head_committer_date() -> Result<String> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let output = Command::new("git")
        .args(["show", "-s", "--format=%cI", "HEAD"])
        .current_dir(&manifest_dir)
        .output()
        .context("run git show for HEAD committer date")?;

    if !output.status.success() {
        bail!("git show HEAD failed ({}): {}", output.status, String::from_utf8_lossy(&output.stderr));
    }

    let date = String::from_utf8(output.stdout)?.trim().to_string();
    if date.is_empty() {
        bail!("git show HEAD returned empty committer date");
    }
    Ok(date)
}

fn http_agent() -> Result<ureq::Agent> {
    Ok(ureq::AgentBuilder::new().timeout(HTTP_TIMEOUT).user_agent(USER_AGENT).build())
}

fn github_token() -> Option<String> {
    env::var("GITHUB_TOKEN")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| env::var("GH_TOKEN").ok().filter(|s| !s.is_empty()))
}

fn apply_auth(req: ureq::Request) -> ureq::Request {
    match github_token() {
        Some(token) => req.set("Authorization", &format!("Bearer {token}")),
        None => req,
    }
}

#[derive(Debug, Deserialize)]
struct GithubCommit {
    sha: String,
}

fn resolve_configs_commit(agent: &ureq::Agent, until_iso: &str) -> Result<String> {
    let url = format!(
        "https://api.github.com/repos/{CONFIGS_REPO}/commits?until={until}&per_page=1",
        until = urlencoding_until(until_iso)
    );
    let req = apply_auth(agent.get(&url).set("Accept", "application/vnd.github+json"));
    let response = req.call().with_context(|| format!("GET {url}"))?;
    let commits: Vec<GithubCommit> = response.into_json().context("decode commits JSON")?;
    let Some(commit) = commits.into_iter().next() else {
        bail!("no {CONFIGS_REPO} commit at or before {until_iso}");
    };
    Ok(commit.sha)
}

/// Percent-encode only what GitHub's `until` query needs (ISO-8601 is mostly safe).
fn urlencoding_until(until_iso: &str) -> String {
    until_iso.replace('+', "%2B")
}

fn download_snapshot(agent: &ureq::Agent, sha: &str, network: &str) -> Result<Vec<u8>> {
    let url = format!(
        "https://raw.githubusercontent.com/{CONFIGS_REPO}/{sha}/network/{network}/cardano-node/peer-snapshot.json"
    );
    let req = apply_auth(agent.get(&url));
    let response = req.call().with_context(|| format!("GET {url}"))?;
    let mut bytes = Vec::new();
    response.into_reader().read_to_end(&mut bytes).context("read snapshot body")?;
    if bytes.is_empty() {
        bail!("empty response body");
    }
    // Cheap sanity check so we do not stage HTML error pages.
    if !bytes.contains(&b'{') {
        bail!("response does not look like JSON");
    }
    Ok(bytes)
}

fn staging_path(staging_root: &Path, network: &str) -> PathBuf {
    staging_root.join(network).join("peer-snapshot.json")
}

fn configs_commit_path(staging_root: &Path) -> PathBuf {
    staging_root.join("CONFIGS_COMMIT")
}

fn read_staged_configs_commit(staging_root: &Path) -> Option<String> {
    fs::read_to_string(configs_commit_path(staging_root)).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn write_staged_configs_commit(staging_root: &Path, sha: &str) -> Result<()> {
    fs::create_dir_all(staging_root)?;
    fs::write(configs_commit_path(staging_root), format!("{sha}\n"))?;
    Ok(())
}

fn write_embed_module(out_dir: &Path, present: &[&str], configs_sha: Option<&str>) -> Result<()> {
    let mut arms = String::new();
    for network in peer_snapshot_network_names() {
        if present.iter().any(|p| p == &network) {
            arms.push_str(&format!(
                "        \"{network}\" => Some(include_bytes!(\"peer_snapshot_{network}.json\")),\n"
            ));
        }
    }

    let sha_lit = match configs_sha {
        Some(sha) => format!("Some(\"{sha}\")"),
        None => "None".to_string(),
    };

    let contents = format!(
        r#"// @generated by build/peer_snapshot.rs — do not edit
#[allow(dead_code)]
pub const CONFIGS_COMMIT: Option<&'static str> = {sha_lit};

pub fn embedded_peer_snapshot(network: &str) -> Option<&'static [u8]> {{
    match network {{
{arms}        _ => None,
    }}
}}
"#
    );

    write_if_changed(&out_dir.join("embedded_peer_snapshots.rs"), &contents)
}

fn env_flag(name: &str) -> bool {
    match env::var(name) {
        Ok(v) => matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"),
        Err(_) => false,
    }
}
