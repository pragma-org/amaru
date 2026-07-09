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
    path::PathBuf,
    process::{Command, Stdio},
};

use amaru_kernel::NetworkName;
use clap::Parser;
use tracing::info;

use super::create::{ManifestEntry, manifest_path, repo_root};

#[derive(Debug, Parser)]
pub struct Args {
    /// The target network.
    #[arg(
        long,
        value_name = amaru::value_names::NETWORK,
        env = amaru::env_vars::NETWORK,
    )]
    network: NetworkName,

    /// Starting epoch of the three-epoch window to publish. Defaults to the latest unpublished
    /// epoch in the manifest.
    #[arg(long, value_name = amaru::value_names::UINT)]
    epoch: Option<u64>,

    /// S3-compatible bucket name.
    #[arg(long, env = "BUCKET_NAME")]
    bucket: String,

    /// S3-compatible endpoint URL (e.g. https://<id>.r2.cloudflarestorage.com).
    #[arg(long, env = "ENDPOINT")]
    endpoint: String,

    /// Public base URL at which uploaded objects are reachable (e.g. https://pub-xxx.r2.dev).
    /// If omitted, inferred from the first existing URL in the manifest.
    #[arg(long, env = "PUBLIC_URL_BASE")]
    public_url_base: Option<String>,
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let Args { network, epoch, bucket, endpoint, public_url_base } = args;

    let manifest_path = manifest_path(network);
    let snapshot_root = repo_root().join(format!("snapshots/{}", network.to_string().to_lowercase()));

    let mut entries: Vec<ManifestEntry> = serde_json::from_slice(&fs::read(&manifest_path)?)?;

    let start_epoch = match epoch {
        Some(e) => e,
        None => {
            let unpublished: std::collections::BTreeSet<u64> = entries
                .iter()
                .filter(|e| e.url.as_deref().unwrap_or("").is_empty())
                .map(|e| u64::from(e.epoch))
                .collect();

            unpublished
                .iter()
                .copied()
                .filter(|&e| unpublished.contains(&(e + 1)) && unpublished.contains(&(e + 2)))
                .max()
                .ok_or("no complete unpublished 3-epoch window found in manifest; run `amaru snapshot create` first")?
        }
    };

    let target_epochs = [start_epoch, start_epoch + 1, start_epoch + 2];

    let public_base = match public_url_base {
        Some(base) => base.trim_end_matches('/').to_owned(),
        None => entries
            .iter()
            .find_map(|e| {
                e.url
                    .as_deref()
                    .filter(|url| !url.is_empty())
                    .and_then(|url| url.rsplit_once('/').map(|(base, _)| base.to_owned()))
            })
            .ok_or("cannot infer public URL base: no existing URLs in manifest. Pass --public-url-base explicitly.")?,
    };

    info!(%network, start_epoch, public_base, "publishing bootstrap snapshots");

    let client = reqwest::Client::new();

    for target_epoch in target_epochs {
        let entry = entries
            .iter()
            .find(|e| u64::from(e.epoch) == target_epoch)
            .ok_or_else(|| format!("epoch {target_epoch} not found in manifest; run `amaru snapshot create` first"))?;

        let archive_name = format!("{}.tar.zst", entry.point);
        let archive_path = snapshot_root.join(&archive_name);
        let object_url = format!("{}/{}", public_base, archive_name);

        if !archive_path.is_file() {
            return Err(
                format!("archive {} not found; run `amaru snapshot create` first", archive_path.display()).into()
            );
        }

        if is_reachable(&client, &object_url).await {
            info!(epoch = target_epoch, url = %object_url, "snapshot already published, skipping upload");
        } else {
            info!(epoch = target_epoch, archive = %archive_path.display(), "uploading snapshot");
            upload_to_s3(&archive_path, &bucket, &archive_name, &endpoint)?;
        }

        if !is_reachable(&client, &object_url).await {
            return Err(format!(
                "uploaded snapshot is not publicly reachable at {object_url}; check bucket permissions and --public-url-base"
            )
            .into());
        }

        info!(epoch = target_epoch, url = %object_url, "snapshot published");

        if let Some(entry) = entries.iter_mut().find(|e| u64::from(e.epoch) == target_epoch) {
            entry.url = Some(object_url);
        }
    }

    let tmp_path = manifest_path.with_extension("json.tmp");
    fs::write(&tmp_path, serde_json::to_vec_pretty(&entries)?)?;
    fs::rename(&tmp_path, &manifest_path)?;

    info!(path = %manifest_path.display(), "updated manifest with published URLs");

    Ok(())
}

async fn is_reachable(client: &reqwest::Client, url: &str) -> bool {
    client.head(url).send().await.map(|r| r.status().is_success() || r.status().as_u16() == 206).unwrap_or(false)
}

fn upload_to_s3(
    archive_path: &PathBuf,
    bucket: &str,
    object_key: &str,
    endpoint: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let s3_uri = format!("s3://{bucket}/{object_key}");
    let status = Command::new("aws")
        .args(["s3", "cp"])
        .arg(archive_path)
        .arg(&s3_uri)
        .arg("--endpoint-url")
        .arg(endpoint)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;

    if !status.success() {
        return Err(format!("aws s3 cp failed with status {status}").into());
    }

    Ok(())
}
