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

use std::{collections::BTreeSet, fs, path::PathBuf};

use amaru::aws::{DEFAULT_BUCKET, DEFAULT_ENDPOINT, DEFAULT_PUBLIC_URL, DEFAULT_REGION, S3Client, S3Config};
use amaru_kernel::NetworkName;
use clap::Parser;
use tracing::info;

use super::create::default_snapshot_output_dir;

const ARCHIVE_EXTENSION: &str = ".tar.zst";

#[derive(Debug, Parser)]
pub struct Args {
    /// The target network.
    #[arg(
        long,
        value_name = amaru::value_names::NETWORK,
        env = amaru::env_vars::NETWORK,
    )]
    network: NetworkName,

    /// Directory containing the local snapshot archives to publish.
    ///
    /// Defaults to ./snapshots/<NETWORK>/ (same as `amaru snapshot create`).
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::SNAPSHOTS_DIR,
    )]
    snapshot_dir: Option<PathBuf>,

    /// S3-compatible bucket name.
    #[arg(long, env = "AMARU_S3_BUCKET", default_value = DEFAULT_BUCKET)]
    bucket: String,

    /// S3-compatible endpoint URL (e.g. https://<id>.r2.cloudflarestorage.com).
    #[arg(long, env = "AMARU_S3_ENDPOINT", default_value = DEFAULT_ENDPOINT)]
    endpoint: String,

    /// S3 region (use "auto" for Cloudflare R2).
    #[arg(long, env = "AMARU_S3_REGION", default_value = DEFAULT_REGION)]
    region: String,

    /// AWS / R2 access key ID for upload authentication.
    #[arg(long, env = "AWS_ACCESS_KEY_ID")]
    aws_access_key_id: String,

    /// AWS / R2 secret access key for upload authentication.
    #[arg(long, env = "AWS_SECRET_ACCESS_KEY")]
    aws_secret_access_key: String,

    /// Public base URL at which uploaded objects are reachable (e.g. https://pub-xxx.r2.dev).
    #[arg(long, env = "AMARU_S3_PUBLIC_URL", default_value = DEFAULT_PUBLIC_URL)]
    public_url: String,
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let Args { network, snapshot_dir, bucket, endpoint, region, aws_access_key_id, aws_secret_access_key, public_url } =
        args;

    let snapshot_root = snapshot_dir.unwrap_or_else(|| default_snapshot_output_dir(network));

    let s3_config = S3Config { bucket, endpoint, region, public_url };
    let s3 = S3Client::new_with_credentials(s3_config, &aws_access_key_id, &aws_secret_access_key);

    // Collect all .tar.zst archives present locally (if the directory exists).
    let local_archives: BTreeSet<String> = fs::read_dir(&snapshot_root)
        .map(|rd| {
            rd.filter_map(|entry| {
                let name = entry.ok()?.file_name().into_string().ok()?;
                name.ends_with(ARCHIVE_EXTENSION).then_some(name)
            })
            .collect()
        })
        .unwrap_or_default();

    // List what is already in S3 under this network prefix.
    let network_prefix = network.to_string().to_lowercase();
    let remote_keys: BTreeSet<String> =
        s3.list_snapshots(network).await?.into_iter().map(|s| format!("{}{ARCHIVE_EXTENSION}", s.point)).collect();

    if local_archives.is_empty() && remote_keys.is_empty() {
        return Err("no archives found locally or in S3; run `amaru snapshot create` first".into());
    }

    info!(%network, local = local_archives.len(), remote = remote_keys.len(), "publishing bootstrap snapshots");

    for archive_name in &local_archives {
        let object_key = format!("{network_prefix}/{archive_name}");
        let archive_path = snapshot_root.join(archive_name);

        if remote_keys.contains(archive_name.as_str()) {
            info!(key = %object_key, "already in S3, skipping");
        } else {
            info!(archive = %archive_path.display(), key = %object_key, "uploading");
            s3.upload_object(&archive_path, &object_key).await?;
            info!(key = %object_key, "uploaded");
        }
    }

    // Update the per-network index so bootstrap can discover snapshots without S3 listing.
    let all_points: Vec<String> = {
        let all_archives: BTreeSet<&String> = remote_keys.union(&local_archives).collect();
        let mut pts: Vec<String> =
            all_archives.iter().filter_map(|name| name.strip_suffix(ARCHIVE_EXTENSION).map(str::to_owned)).collect();
        pts.sort();
        pts
    };
    let index_json = serde_json::to_vec_pretty(&all_points)?;
    s3.upload_bytes(index_json, &format!("{network_prefix}/index.json")).await?;
    info!(%network, snapshots = all_points.len(), "updated S3 index");

    Ok(())
}
