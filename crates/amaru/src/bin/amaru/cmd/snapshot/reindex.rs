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

use std::env;

use amaru::{
    aws::{DEFAULT_BUCKET, DEFAULT_ENDPOINT, DEFAULT_PUBLIC_URL, DEFAULT_REGION, S3Client, S3Config},
    lifecycle::{Runnable, RuntimeKind},
};
use amaru_kernel::NetworkName;
use amaru_observability::info;
use anyhow::anyhow;
use clap::Parser;

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
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Io, move || run(args))
}

async fn run(args: Args) -> anyhow::Result<()> {
    let Args { network, s3_bucket, s3_endpoint, s3_region } = args;
    let aws_access_key_id = required_env(AWS_ACCESS_KEY_ID_ENV)?;
    let aws_secret_access_key = required_env(AWS_SECRET_ACCESS_KEY_ENV)?;
    let s3_config = S3Config {
        bucket: s3_bucket,
        endpoint: s3_endpoint,
        region: s3_region,
        public_url: DEFAULT_PUBLIC_URL.to_owned(),
    };
    let s3 = S3Client::new_with_credentials(s3_config, aws_access_key_id, aws_secret_access_key);
    let snapshots = s3.list_snapshot_objects(network).await?;
    let points: Vec<&str> = snapshots.iter().map(|snapshot| snapshot.point.as_str()).collect();
    let network_prefix = network.to_string().to_lowercase();

    s3.upload_bytes(serde_json::to_vec_pretty(&points)?, &format!("{network_prefix}/index.json")).await?;
    info!(cli::snapshot::UPDATE_INDEX, %network, snapshots = points.len());

    Ok(())
}

fn required_env(name: &str) -> anyhow::Result<String> {
    match env::var(name) {
        Ok(value) if value.is_empty() => {
            Err(anyhow!("environment variable {name} is empty; set it before running `amaru snapshot reindex`"))
        }
        Ok(value) => Ok(value),
        Err(env::VarError::NotPresent) => {
            Err(anyhow!("missing required environment variable {name}; set it before running `amaru snapshot reindex`"))
        }
        Err(env::VarError::NotUnicode(_)) => Err(anyhow!("environment variable {name} must contain valid UTF-8")),
    }
}
