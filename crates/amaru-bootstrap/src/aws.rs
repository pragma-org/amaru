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
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use amaru_kernel::{NetworkName, NetworkPoint};
use amaru_observability::info;
use amaru_progress_bar::{ProgressBar, TerminalProgressBar};
use anyhow::{Context, anyhow};
use aws_credential_types::{Credentials, provider::SharedCredentialsProvider};
use aws_sdk_s3::{
    Client,
    config::{BehaviorVersion, Builder, Region, RequestChecksumCalculation},
    primitives::{ByteStream, SdkBody},
};
use http_body_util::BodyExt as _;
use reqwest::header::CONTENT_LENGTH;

/// Default S3 bucket name for Amaru bootstrap snapshots.
pub const DEFAULT_BUCKET: &str = "cardano-ledger-snapshots";

/// Default S3-compatible endpoint for Cloudflare R2 (authenticated API).
pub const DEFAULT_ENDPOINT: &str = "https://bd7a351a104d485dcc8b921c273db90c.r2.cloudflarestorage.com";

/// Default public CDN URL for Cloudflare R2 (anonymous read access).
pub const DEFAULT_PUBLIC_URL: &str = "https://pub-b844360df4774bb092a2bb2043b888e5.r2.dev";

/// Default S3 region (Cloudflare R2 uses "auto").
pub const DEFAULT_REGION: &str = "auto";

pub const ARCHIVE_EXTENSION: &str = ".tar.zst";

/// Configuration for an S3 (or S3-compatible) client.
#[derive(Clone, Debug)]
pub struct S3Config {
    pub bucket: String,
    pub endpoint: String,
    pub region: String,
    /// Public CDN base URL for anonymous read access (e.g. <https://pub-xxx.r2.dev>).
    pub public_url: String,
}

impl Default for S3Config {
    fn default() -> Self {
        S3Config {
            bucket: DEFAULT_BUCKET.to_owned(),
            endpoint: DEFAULT_ENDPOINT.to_owned(),
            region: DEFAULT_REGION.to_owned(),
            public_url: DEFAULT_PUBLIC_URL.to_owned(),
        }
    }
}

/// Thin wrapper around the AWS S3 client, scoped to a specific bucket and endpoint.
pub struct S3Client {
    inner: Client,
    config: S3Config,
}

impl S3Client {
    /// Create an authenticated S3 client for upload access.
    pub fn new_with_credentials(
        config: S3Config,
        access_key: impl Into<String>,
        secret_key: impl Into<String>,
    ) -> Self {
        let access_key = access_key.into();
        let secret_key = secret_key.into();
        let creds = Credentials::new(access_key, secret_key, None, None, "amaru");
        let provider = SharedCredentialsProvider::new(creds);

        let sdk_config = Builder::new()
            .endpoint_url(&config.endpoint)
            .region(Region::new(config.region.clone()))
            .credentials_provider(provider)
            .force_path_style(true)
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .behavior_version(BehaviorVersion::latest())
            .build();

        S3Client { inner: Client::from_conf(sdk_config), config }
    }

    /// List snapshot points (`<slot>.<hash>`) stored under the network prefix, independently from `index.json`.
    pub async fn list_snapshot_objects(&self, network: NetworkName) -> anyhow::Result<Vec<String>> {
        let prefix = format!("{}/", network.to_string().to_lowercase());
        let mut continuation_token = None;
        let mut snapshots = Vec::new();

        loop {
            let response = self
                .inner
                .list_objects_v2()
                .bucket(&self.config.bucket)
                .prefix(&prefix)
                .set_continuation_token(continuation_token)
                .send()
                .await?;

            snapshots.extend(
                response
                    .contents()
                    .iter()
                    .filter_map(|object| object.key())
                    .filter_map(|key| parse_snapshot_key(key, &prefix)),
            );

            if !response.is_truncated().unwrap_or(false) {
                break;
            }

            continuation_token = response.next_continuation_token().map(str::to_owned);
            if continuation_token.is_none() {
                return Err(anyhow!("S3 returned a truncated object listing without a continuation token"));
            }
        }

        snapshots.sort();
        Ok(snapshots)
    }

    /// Upload a local file to S3 at the given key.
    pub async fn upload_object(&self, src: &Path, key: &str) -> anyhow::Result<()> {
        let size = tokio::fs::metadata(src).await?.len();
        let content_length = i64::try_from(size)?;
        let progress = Arc::new(transfer_progress_bar("Uploading", size));
        let maximum_progress = Arc::new(AtomicU64::new(0));
        let body = ByteStream::read_from().path(src).build().await?.into_inner().map_preserve_contents({
            let progress = Arc::clone(&progress);
            move |body| {
                let progress = Arc::clone(&progress);
                let maximum_progress = Arc::clone(&maximum_progress);
                let mut attempt_progress = 0;
                SdkBody::from_body_1_x(body.map_frame(move |frame| {
                    if let Some(bytes) = frame.data_ref() {
                        attempt_progress += bytes.len() as u64;
                        let previous = maximum_progress.fetch_max(attempt_progress, Ordering::Relaxed);
                        if attempt_progress > previous {
                            progress.tick((attempt_progress - previous) as usize);
                        }
                    }
                    frame
                }))
            }
        });

        let result = self
            .inner
            .put_object()
            .bucket(&self.config.bucket)
            .key(key)
            .content_length(content_length)
            .body(ByteStream::new(body))
            .send()
            .await
            .map(|_| ())
            .map_err(Into::into);
        if result.is_ok() {
            progress.finish_shared(|| {
                info!(cli::snapshot::UPLOADED, archive = src.display().to_string());
            });
        } else {
            progress.clear_shared();
        }
        result
    }

    /// Upload raw bytes to S3 at the given key.
    pub async fn upload_bytes(&self, bytes: Vec<u8>, key: &str) -> anyhow::Result<()> {
        let body = ByteStream::from(bytes);

        self.inner.put_object().bucket(&self.config.bucket).key(key).body(body).send().await?;

        Ok(())
    }
}

fn parse_snapshot_key(key: &str, prefix: &str) -> Option<String> {
    let filename = key.strip_prefix(prefix)?;
    let point = filename.strip_suffix(ARCHIVE_EXTENSION)?;

    if point.contains('/')
        || point.split('.').count() != 2
        || matches!(NetworkPoint::try_from(point).ok()?, NetworkPoint::Origin)
    {
        return None;
    }

    Some(point.to_owned())
}

/// S3 client for unauthenticated (public-read) access via the public CDN URL.
///
/// Fetches `<network>/index.json` (written by `publish`) for listing, and downloads
/// individual archives via plain unsigned HTTP. The CDN URL (e.g. `pub-xxx.r2.dev`)
/// allows anonymous GET without AWS SigV4 signing, unlike the S3 API endpoint.
pub struct AnonymousS3Client {
    http: reqwest::Client,
    /// Base URL: `<public_url>/<bucket>`, e.g. `https://pub-xxx.r2.dev/cardano-ledger-snapshots`
    base_url: String,
}

impl AnonymousS3Client {
    pub fn new(config: S3Config) -> Self {
        let base_url = config.public_url.trim_end_matches('/').to_owned();
        AnonymousS3Client { http: reqwest::Client::new(), base_url }
    }

    /// List available snapshots by fetching `<network>/index.json` from the public CDN.
    ///
    /// Returns an empty list if the index does not exist yet (no snapshots published).
    pub async fn list_snapshots(&self, network: NetworkName) -> anyhow::Result<Vec<String>> {
        let network_prefix = network.to_string().to_lowercase();
        let url = format!("{}/{network_prefix}/index.json", self.base_url);

        let response = self.http.get(&url).send().await?;
        if response.status().as_u16() == 404 {
            return Ok(Vec::new());
        }

        Ok(response.error_for_status()?.json().await?)
    }

    /// Return the object size advertised by the public endpoint, when available.
    pub(crate) async fn object_size(&self, key: &str) -> anyhow::Result<Option<u64>> {
        let url = format!("{}/{key}", self.base_url);
        let response = self.http.head(&url).send().await?.error_for_status()?;
        Ok(response.headers().get(CONTENT_LENGTH).and_then(|value| value.to_str().ok()?.parse().ok()))
    }

    /// Download an object while reporting the size of each written chunk.
    pub(crate) async fn download_object_with_progress(
        &self,
        key: &str,
        dest: &Path,
        mut on_bytes: impl FnMut(u64),
    ) -> anyhow::Result<()> {
        use futures_util::TryStreamExt as _;
        use tokio::{fs::File, io::AsyncWriteExt as _};

        let url = format!("{}/{key}", self.base_url);
        let response = self.http.get(&url).send().await?.error_for_status().context("download failed")?;
        let mut stream = response.bytes_stream();

        let mut file = File::create(dest).await?;
        while let Some(chunk) = stream.try_next().await? {
            file.write_all(&chunk).await?;
            on_bytes(chunk.len() as u64);
        }
        file.sync_all().await?;
        Ok(())
    }
}

fn transfer_progress_bar(action: &str, size: u64) -> TerminalProgressBar {
    let progress = TerminalProgressBar::new(
        size,
        format!(
            "{{spinner:.green}} {action} {{bytes_per_sec:>10}} {{bar:40.green}} [{{bytes:>10}}/{{total_bytes:<10}}] ({{eta}} remaining)"
        ),
    );
    progress.refresh();
    progress
}
