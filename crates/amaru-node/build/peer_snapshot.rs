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
//! ## Cargo rebuild hygiene
//!
//! The only package-tree inputs this script tells Cargo to watch are the optional
//! staged offline files:
//!
//! ```text
//! config/peer-snapshots/<network>/peer-snapshot.json
//! ```
//!
//! Fetch metadata (`CONFIGS_COMMIT_CACHE`) and downloaded snapshot bytes live under
//! `OUT_DIR` only. Writing them must never dirty a `rerun-if-changed` path, or every
//! successful fetch / token env flip would recompile `amaru-node`.
//!
//! Credentials and `AMARU_SKIP_PEER_SNAPSHOT_FETCH` are read when the script runs but
//! are **not** listed as `rerun-if-env-changed`: they only affect best-effort network
//! access, not the embedded bytes once `OUT_DIR` / staged inputs are stable.
//!
//! ## Fetch behaviour
//!
//! When GitHub is reachable, snapshots are downloaded from
//! `cardano-foundation/cardano-configurations` at the youngest commit at or before the
//! Amaru HEAD committer timestamp into `OUT_DIR`. Commit metadata is cached in
//! `OUT_DIR/CONFIGS_COMMIT_CACHE` for [`COMMIT_CACHE_TTL`] so frequent local rebuilds
//! do not spam the commits API.
//!
//! Every network in [`PEER_SNAPSHOT_NETWORKS`] must end up with bytes in `OUT_DIR`
//! (from download and/or staged offline files). See
//! `config/peer-snapshots/README.md`.

use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime},
};

use amaru_kernel::PEER_SNAPSHOT_NETWORKS;
use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::{emit_rerun_if_exists, write_if_changed, write_if_changed_bytes};

const CONFIGS_REPO: &str = "cardano-foundation/cardano-configurations";
const USER_AGENT: &str = "amaru-build (https://github.com/pragma-org/amaru)";
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const COMMIT_CACHE_TTL: Duration = Duration::from_secs(12 * 60 * 60);
const UNTIL_CACHE_TOLERANCE: Duration = Duration::from_secs(60 * 60);
const CACHE_FILE_NAME: &str = "CONFIGS_COMMIT_CACHE";

fn peer_snapshot_network_names() -> impl Iterator<Item = &'static str> {
    PEER_SNAPSHOT_NETWORKS.iter().map(|n| n.as_str())
}

/// Stage peer snapshots and embed them into `OUT_DIR`.
///
/// Fails if any network in [`PEER_SNAPSHOT_NETWORKS`] still lacks embeddable bytes
/// after the fetch attempt and staged-file fallback.
pub fn prepare_peer_snapshots() -> Result<()> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").context("CARGO_MANIFEST_DIR")?);
    let out_dir = PathBuf::from(env::var("OUT_DIR").context("OUT_DIR")?);
    let staging_root = manifest_dir.join("config").join("peer-snapshots");

    // Content inputs only: optional offline/CI staged snapshots. Never watch fetch
    // caches or anything this script writes under OUT_DIR.
    for network in peer_snapshot_network_names() {
        emit_rerun_if_exists(&staging_path(&staging_root, network));
    }

    let skip_fetch = env_flag("AMARU_SKIP_PEER_SNAPSHOT_FETCH");

    let mut configs_sha: Option<String> = read_configs_commit_cache(&out_dir).map(|c| c.sha);
    if !skip_fetch {
        match fetch_all(&out_dir) {
            Ok(sha) => {
                configs_sha = Some(sha);
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
        let dest = out_dir_snapshot_path(&out_dir, network);
        if dest.is_file() {
            present.push(network);
            continue;
        }

        // Offline / CI fallback: copy staged package inputs into OUT_DIR.
        let staged = staging_path(&staging_root, network);
        if staged.is_file() {
            let bytes = fs::read(&staged).with_context(|| format!("read staged {}", staged.display()))?;
            write_if_changed_bytes(&dest, &bytes)
                .with_context(|| format!("copy staged {} -> {}", staged.display(), dest.display()))?;
            present.push(network);
        } else {
            missing.push(network);
        }
    }

    if !missing.is_empty() {
        bail!(
            "no embeddable peer snapshot(s) for network(s): {}. \
             Stage files under config/peer-snapshots/<network>/peer-snapshot.json \
             (empty placeholders work offline), or allow the build script to fetch. \
             See crates/amaru-node/config/peer-snapshots/README.md",
            missing.join(", ")
        );
    }

    write_embed_module(&out_dir, &present, configs_sha.as_deref())?;
    Ok(())
}

/// Download snapshots into `out_dir`, using an OUT_DIR-local API cache.
fn fetch_all(out_dir: &Path) -> Result<String> {
    let amaru_time = amaru_head_committer_time().context("determine Amaru HEAD committer date")?;
    let existing = read_configs_commit_cache(out_dir);

    // Skip the commits API while the OUT_DIR cache is younger than COMMIT_CACHE_TTL
    // and was resolved for a sufficiently close Amaru HEAD committer time.
    let (sha, cache_for_write) = match existing.as_ref() {
        Some(cache) if commit_cache_is_reusable(out_dir, cache, &amaru_time) => (cache.sha.clone(), None),
        _ => {
            let agent = http_agent()?;
            // Conditional headers are only valid for the same logical `until` query.
            let conditional = existing.as_ref().filter(|c| until_is_compatible(c, &amaru_time));
            let resolved = resolve_configs_commit(&agent, &amaru_time.iso, conditional)
                .with_context(|| format!("resolve {CONFIGS_REPO} commit at or before {}", amaru_time.iso))?;
            let cache = match resolved {
                ResolveResult::NotModified => {
                    let mut cache =
                        existing.clone().context("received HTTP 304 without a staged configs commit cache")?;
                    cache.until = Some(amaru_time.unix);
                    cache
                }
                ResolveResult::Updated { mut cache } => {
                    cache.until = Some(amaru_time.unix);
                    cache
                }
            };
            let sha = cache.sha.clone();
            (sha, Some(cache))
        }
    };

    let previous_sha = existing.as_ref().map(|c| c.sha.as_str());
    let sha_changed = previous_sha != Some(sha.as_str());

    let agent = http_agent()?;
    for network in peer_snapshot_network_names() {
        let dest = out_dir_snapshot_path(out_dir, network);
        if !sha_changed && dest.is_file() {
            continue;
        }
        match download_snapshot(&agent, &sha, network) {
            Ok(bytes) => {
                write_if_changed_bytes(&dest, &bytes)
                    .with_context(|| format!("write downloaded snapshot for {network}"))?;
            }
            Err(err) => {
                println!(
                    "cargo:warning=failed to download peer snapshot for {network} at {CONFIGS_REPO}@{sha}: {err:#}"
                );
            }
        }
    }

    if let Some(cache) = cache_for_write {
        write_configs_commit_cache(out_dir, &cache)?;
    }

    Ok(sha)
}

/// True when the cache may be reused without contacting the commits API.
fn commit_cache_is_reusable(out_dir: &Path, cache: &ConfigsCommitCache, amaru_time: &AmaruHeadTime) -> bool {
    commit_cache_mtime_is_fresh(out_dir) && until_is_compatible(cache, amaru_time)
}

/// True when the OUT_DIR `CONFIGS_COMMIT_CACHE` exists and was modified within [`COMMIT_CACHE_TTL`].
fn commit_cache_mtime_is_fresh(out_dir: &Path) -> bool {
    let path = configs_commit_cache_path(out_dir);
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    let Ok(mtime) = meta.modified() else {
        return false;
    };
    match SystemTime::now().duration_since(mtime) {
        Ok(age) => age < COMMIT_CACHE_TTL,
        // Clock skew / future mtime: treat as stale so we re-resolve.
        Err(_) => false,
    }
}

/// True when the cached resolve used an Amaru HEAD time within [`UNTIL_CACHE_TOLERANCE`]
/// of the currently required one.
fn until_is_compatible(cache: &ConfigsCommitCache, amaru_time: &AmaruHeadTime) -> bool {
    let Some(cached_unix) = cache.until else {
        // Pre-`until` cache files are not reusable for timestamp-sensitive resolves.
        return false;
    };
    amaru_time.unix.abs_diff(cached_unix) <= UNTIL_CACHE_TOLERANCE.as_secs()
}

/// Amaru HEAD committer time: ISO-8601 for the GitHub `until` query, unix for comparisons.
struct AmaruHeadTime {
    iso: String,
    unix: i64,
}

fn amaru_head_committer_time() -> Result<AmaruHeadTime> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let output = Command::new("git")
        .args(["show", "-s", "--format=%cI%n%ct", "HEAD"])
        .current_dir(&manifest_dir)
        .output()
        .context("run git show for HEAD committer date")?;

    if !output.status.success() {
        bail!("git show HEAD failed ({}): {}", output.status, String::from_utf8_lossy(&output.stderr));
    }

    let text = String::from_utf8(output.stdout)?;
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let iso = lines.next().unwrap_or_default().to_string();
    let unix_str = lines.next().unwrap_or_default();
    if iso.is_empty() || unix_str.is_empty() {
        bail!("git show HEAD returned incomplete committer date (need %cI and %ct)");
    }
    let unix: i64 = unix_str.parse().with_context(|| format!("parse git committer unix timestamp {unix_str:?}"))?;
    Ok(AmaruHeadTime { iso, unix })
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

#[derive(Debug, Clone)]
struct ConfigsCommitCache {
    sha: String,
    /// Amaru HEAD committer time (unix seconds) used as GitHub `until` when resolving `sha`.
    until: Option<i64>,
    etag: Option<String>,
    last_modified: Option<String>,
}

enum ResolveResult {
    NotModified,
    Updated { cache: ConfigsCommitCache },
}

fn resolve_configs_commit(
    agent: &ureq::Agent,
    until_iso: &str,
    existing: Option<&ConfigsCommitCache>,
) -> Result<ResolveResult> {
    let url = format!(
        "https://api.github.com/repos/{CONFIGS_REPO}/commits?until={until}&per_page=1",
        until = urlencoding_until(until_iso)
    );
    let mut req = apply_auth(agent.get(&url).set("Accept", "application/vnd.github+json"));
    if let Some(cache) = existing {
        if let Some(etag) = &cache.etag {
            req = req.set("If-None-Match", etag);
        }
        if let Some(last_modified) = &cache.last_modified {
            req = req.set("If-Modified-Since", last_modified);
        }
    }

    // ureq returns Ok for 304 (empty body); also handle Error::Status(304) defensively.
    match req.call() {
        Ok(response) if response.status() == 304 => not_modified(existing),
        Ok(response) => {
            let etag = response.header("etag").map(str::to_string);
            let last_modified = response.header("last-modified").map(str::to_string);
            let commits: Vec<GithubCommit> = response.into_json().context("decode commits JSON")?;
            let Some(commit) = commits.into_iter().next() else {
                bail!("no {CONFIGS_REPO} commit at or before {until_iso}");
            };
            Ok(ResolveResult::Updated {
                cache: ConfigsCommitCache { sha: commit.sha, until: None, etag, last_modified },
            })
        }
        Err(ureq::Error::Status(304, _response)) => not_modified(existing),
        Err(ureq::Error::Status(statue, response)) => {
            bail!("GET {url} returned HTTP {statue} with headers {}", response_headers(&response))
        }
        Err(ureq::Error::Transport(err)) => Err(err).with_context(|| format!("GET {url}")),
    }
}

fn response_headers(response: &ureq::Response) -> String {
    response
        .headers_names()
        .into_iter()
        .map(|name| format!("{}: {}", name, response.header(&name).unwrap_or_default()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn not_modified(existing: Option<&ConfigsCommitCache>) -> Result<ResolveResult> {
    if existing.is_none() {
        bail!("received HTTP 304 without a staged configs commit cache");
    }
    Ok(ResolveResult::NotModified)
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

fn out_dir_snapshot_path(out_dir: &Path, network: &str) -> PathBuf {
    out_dir.join(format!("peer_snapshot_{network}.json"))
}

fn configs_commit_cache_path(out_dir: &Path) -> PathBuf {
    out_dir.join(CACHE_FILE_NAME)
}

fn read_configs_commit_cache(out_dir: &Path) -> Option<ConfigsCommitCache> {
    let content = fs::read_to_string(configs_commit_cache_path(out_dir)).ok()?;
    parse_configs_commit_cache(&content)
}

fn parse_configs_commit_cache(content: &str) -> Option<ConfigsCommitCache> {
    if content.trim().is_empty() {
        return None;
    }

    let mut sha = None;
    let mut until = None;
    let mut etag = None;
    let mut last_modified = None;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match key {
            "sha" => sha = Some(value.to_string()),
            "until" => until = value.parse().ok(),
            "etag" => etag = Some(value.to_string()),
            "last-modified" => last_modified = Some(value.to_string()),
            _ => {}
        }
    }

    Some(ConfigsCommitCache { sha: sha?, until, etag, last_modified })
}

fn write_configs_commit_cache(out_dir: &Path, cache: &ConfigsCommitCache) -> Result<()> {
    fs::create_dir_all(out_dir)?;
    let mut body = format!("sha: {}\n", cache.sha);
    if let Some(until) = cache.until {
        body.push_str(&format!("until: {until}\n"));
    }
    if let Some(etag) = &cache.etag {
        body.push_str(&format!("etag: {etag}\n"));
    }
    if let Some(last_modified) = &cache.last_modified {
        body.push_str(&format!("last-modified: {last_modified}\n"));
    }
    // OUT_DIR is not cargo-watched; unconditional write is fine and refreshes the API TTL.
    fs::write(configs_commit_cache_path(out_dir), body)?;
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
