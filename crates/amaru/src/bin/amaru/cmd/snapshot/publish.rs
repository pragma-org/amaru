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

use std::{collections::BTreeSet, env, fs, path::PathBuf};

use amaru::{
    aws::{DEFAULT_BUCKET, DEFAULT_ENDPOINT, DEFAULT_PUBLIC_URL, DEFAULT_REGION, S3Client, S3Config},
    bootstrap::validate_publishable_snapshot_archive,
    lifecycle::{Runnable, RuntimeKind},
};
use amaru_kernel::{NetworkName, NetworkPoint, utils::path::relative_path};
use amaru_observability::info;
use anyhow::{Context, anyhow};
use clap::Parser;

use super::create::default_snapshot_output_dir;

const ARCHIVE_EXTENSION: &str = ".tar.zst";
const AWS_ACCESS_KEY_ID_ENV: &str = "AWS_ACCESS_KEY_ID";
const AWS_SECRET_ACCESS_KEY_ENV: &str = "AWS_SECRET_ACCESS_KEY";

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
    /// Defaults to snapshots/<NETWORK>/ beside the `amaru` executable.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::SNAPSHOTS_DIR,
    )]
    snapshot_dir: Option<PathBuf>,

    /// S3-compatible bucket name.
    #[arg(
        long,
        value_name = amaru::value_names::BUCKET_NAME,
        env = "AMARU_S3_BUCKET",
        default_value = DEFAULT_BUCKET,
    )]
    s3_bucket: String,

    /// S3-compatible endpoint URL.
    #[arg(
        long,
        value_name = amaru::value_names::URL,
        env = "AMARU_S3_ENDPOINT",
        default_value = DEFAULT_ENDPOINT,
    )]
    s3_endpoint: String,

    /// S3-compatible region.
    #[arg(
        long,
        value_name = amaru::value_names::S3_REGION,
        env = "AMARU_S3_REGION",
        default_value = DEFAULT_REGION,
    )]
    s3_region: String,

    /// Public base URL at which uploaded objects are reachable.
    #[arg(
        long,
        value_name = amaru::value_names::URL,
        env = "AMARU_S3_PUBLIC_URL",
        default_value = DEFAULT_PUBLIC_URL,
    )]
    s3_public_url: String,
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Io, move || run(args))
}

async fn run(args: Args) -> anyhow::Result<()> {
    let Args { network, snapshot_dir, s3_bucket, s3_endpoint, s3_region, s3_public_url } = args;

    let aws_access_key_id = required_env(AWS_ACCESS_KEY_ID_ENV)?;
    let aws_secret_access_key = required_env(AWS_SECRET_ACCESS_KEY_ENV)?;

    let snapshot_root = match snapshot_dir {
        Some(path) => path,
        None => default_snapshot_output_dir(network)?,
    };

    let s3_config = S3Config { bucket: s3_bucket, endpoint: s3_endpoint, region: s3_region, public_url: s3_public_url };
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

    // List the actual objects in S3 under this network prefix, independently from index.json.
    let network_prefix = network.to_string().to_lowercase();
    let remote_keys: BTreeSet<String> = s3
        .list_snapshot_objects(network)
        .await?
        .into_iter()
        .map(|s| format!("{}{ARCHIVE_EXTENSION}", s.point))
        .collect();

    if local_archives.is_empty() && remote_keys.is_empty() {
        anyhow::bail!("no archives found locally or in S3; run `amaru snapshot create` first");
    }

    let archives_to_upload: Vec<&String> = local_archives.difference(&remote_keys).collect();
    for archive_name in &archives_to_upload {
        let archive_path = snapshot_root.join(archive_name);
        let point = snapshot_point_from_archive_name(archive_name)?;
        validate_publishable_snapshot_archive(&archive_path, point)
            .with_context(|| format!("refusing to publish invalid snapshot archive {}", archive_path.display()))?;
    }

    info!(cli::snapshot::PUBLISH, network, local = local_archives.len(), remote = remote_keys.len());

    for archive_name in &local_archives {
        let object_key = format!("{network_prefix}/{archive_name}");
        let archive_path = snapshot_root.join(archive_name);

        if remote_keys.contains(archive_name.as_str()) {
            info!(cli::snapshot::SKIP_UPLOAD, archive = archive_name);
        } else {
            info!(cli::snapshot::UPLOAD, archive = relative_path(&archive_path)?.display().to_string());
            s3.upload_object(&archive_path, &object_key).await?;
            info!(cli::snapshot::UPLOADED, archive = relative_path(&archive_path)?.display().to_string());
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
    info!(cli::snapshot::UPDATE_INDEX, network, snapshots = all_points.len());

    Ok(())
}

fn snapshot_point_from_archive_name(archive_name: &str) -> anyhow::Result<&str> {
    let point = archive_name
        .strip_suffix(ARCHIVE_EXTENSION)
        .ok_or_else(|| anyhow!("snapshot archive must end with {ARCHIVE_EXTENSION}: {archive_name}"))?;
    if point.split('.').count() != 2 || !matches!(NetworkPoint::try_from(point), Ok(NetworkPoint::Specific(_, _))) {
        anyhow::bail!("invalid snapshot archive point in filename: {archive_name}");
    }
    Ok(point)
}

fn required_env(name: &str) -> anyhow::Result<String> {
    match env::var(name) {
        Ok(value) if value.is_empty() => {
            Err(anyhow!("environment variable {name} is empty; set it before running `amaru snapshot publish`"))
        }
        Ok(value) => Ok(value),
        Err(env::VarError::NotPresent) => {
            Err(anyhow!("missing required environment variable {name}; set it before running `amaru snapshot publish`"))
        }
        Err(env::VarError::NotUnicode(_)) => Err(anyhow!("environment variable {name} must contain valid UTF-8")),
    }
}
