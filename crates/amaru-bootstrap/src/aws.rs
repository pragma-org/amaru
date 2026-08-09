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

use amaru_kernel::{NetworkName, Point};
use amaru_progress_bar::{ProgressBar, TerminalProgressBar};
use aws_credential_types::{Credentials, provider::SharedCredentialsProvider};
use aws_sdk_s3::{
    Client,
    config::{BehaviorVersion, Builder, Region, RequestChecksumCalculation},
    primitives::{ByteStream, SdkBody},
};
use http_body_util::BodyExt as _;

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

/// An S3 snapshot entry discovered by listing the bucket.
pub struct S3Snapshot {
    /// The snapshot point in `<slot>.<hash>` format.
    pub point: String,
    /// The full S3 object key: `<network>/<point>.tar.zst`.
    pub key: String,
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

    /// List available snapshots by fetching `<network>/index.json` from S3.
    ///
    /// Returns an empty list if the index does not exist yet (no snapshots published).
    pub async fn list_snapshots(&self, network: NetworkName) -> Result<Vec<S3Snapshot>, Box<dyn std::error::Error>> {
        let network_prefix = network.to_string().to_lowercase();
        let key = format!("{network_prefix}/index.json");

        let response = match self.inner.get_object().bucket(&self.config.bucket).key(&key).send().await {
            Ok(r) => r,
            Err(err) => {
                let is_not_found = err.as_service_error().map(|e| e.is_no_such_key()).unwrap_or(false);
                if is_not_found {
                    return Ok(Vec::new());
                }
                return Err(err.into());
            }
        };

        let bytes = response.body.collect().await?.into_bytes();
        let points: Vec<String> = serde_json::from_slice(&bytes)?;

        Ok(points
            .into_iter()
            .map(|point| S3Snapshot { key: format!("{network_prefix}/{point}{ARCHIVE_EXTENSION}"), point })
            .collect())
    }

    /// List snapshot archives stored under the network prefix, independently from `index.json`.
    pub async fn list_snapshot_objects(
        &self,
        network: NetworkName,
    ) -> Result<Vec<S3Snapshot>, Box<dyn std::error::Error>> {
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
                return Err("S3 returned a truncated object listing without a continuation token".into());
            }
        }

        snapshots.sort_by(|left, right| left.point.cmp(&right.point));
        Ok(snapshots)
    }

    /// Download an S3 object and write it to `dest`.
    pub async fn download_object(&self, key: &str, dest: &Path) -> Result<(), Box<dyn std::error::Error>> {
        use tokio::{fs::File, io::AsyncWriteExt as _};

        let response = self.inner.get_object().bucket(&self.config.bucket).key(key).send().await?;
        let bytes = response.body.collect().await?.into_bytes();

        let mut file = File::create(dest).await?;
        file.write_all(&bytes).await?;
        file.sync_all().await?;

        Ok(())
    }

    /// Upload a local file to S3 at the given key.
    pub async fn upload_object(&self, src: &Path, key: &str) -> Result<(), Box<dyn std::error::Error>> {
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
        progress.clear();
        result
    }

    /// Upload raw bytes to S3 at the given key.
    pub async fn upload_bytes(&self, bytes: Vec<u8>, key: &str) -> Result<(), Box<dyn std::error::Error>> {
        let body = ByteStream::from(bytes);

        self.inner.put_object().bucket(&self.config.bucket).key(key).body(body).send().await?;

        Ok(())
    }

    /// Return `true` if the S3 object with `key` exists, `false` if it returns 404.
    pub async fn object_exists(&self, key: &str) -> Result<bool, Box<dyn std::error::Error>> {
        match self.inner.head_object().bucket(&self.config.bucket).key(key).send().await {
            Ok(_) => Ok(true),
            Err(err) => {
                let is_not_found = err.as_service_error().map(|e| e.is_not_found()).unwrap_or(false);
                if is_not_found { Ok(false) } else { Err(err.into()) }
            }
        }
    }
}

fn parse_snapshot_key(key: &str, prefix: &str) -> Option<S3Snapshot> {
    let filename = key.strip_prefix(prefix)?;
    let point = filename.strip_suffix(ARCHIVE_EXTENSION)?;

    if point.contains('/') || point.split('.').count() != 2 || matches!(Point::try_from(point).ok()?, Point::Origin) {
        return None;
    }

    Some(S3Snapshot { point: point.to_owned(), key: key.to_owned() })
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
    pub async fn list_snapshots(&self, network: NetworkName) -> Result<Vec<S3Snapshot>, Box<dyn std::error::Error>> {
        let network_prefix = network.to_string().to_lowercase();
        let url = format!("{}/{network_prefix}/index.json", self.base_url);

        let response = self.http.get(&url).send().await?;
        if response.status().as_u16() == 404 {
            return Ok(Vec::new());
        }

        let points: Vec<String> = response.error_for_status()?.json().await?;

        Ok(points
            .into_iter()
            .map(|point| S3Snapshot { key: format!("{network_prefix}/{point}{ARCHIVE_EXTENSION}"), point })
            .collect())
    }

    /// Download an object by key using an unsigned GET against the public CDN.
    pub async fn download_object(&self, key: &str, dest: &Path) -> Result<(), Box<dyn std::error::Error>> {
        use futures_util::TryStreamExt as _;
        use tokio::{fs::File, io::AsyncWriteExt as _};

        let url = format!("{}/{key}", self.base_url);
        let response =
            self.http.get(&url).send().await?.error_for_status().map_err(|e| format!("download failed: {e}"))?;
        let progress = transfer_progress_bar("Downloading", response.content_length().unwrap_or(0));
        let mut stream = response.bytes_stream();

        let result: Result<(), Box<dyn std::error::Error>> = async {
            let mut file = File::create(dest).await?;
            while let Some(chunk) = stream.try_next().await? {
                file.write_all(&chunk).await?;
                progress.tick(chunk.len());
            }
            file.sync_all().await?;
            Ok(())
        }
        .await;
        progress.clear();
        result
    }
}

fn transfer_progress_bar(action: &str, size: u64) -> TerminalProgressBar {
    let progress = TerminalProgressBar::new(
        size,
        format!(
            "{{spinner:.green}} {action} {{bytes_per_sec:>10}} {{bar:40.green}} [{{bytes:>10}}/{{total_bytes:<10}}] ({{eta}} remaining)"
        ),
    );
    progress.tick(0);
    progress
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_snapshot_key_valid() {
        let prefix = "preprod/";
        let key = "preprod/69206375.6f99b5f3deaeae8dc43fce3db2f3cd36ad8ed174ca3400b5b1bed76fdf248912.tar.zst";
        let snap = parse_snapshot_key(key, prefix).unwrap();
        assert_eq!(snap.point, "69206375.6f99b5f3deaeae8dc43fce3db2f3cd36ad8ed174ca3400b5b1bed76fdf248912");
        assert_eq!(snap.key, key);
    }

    #[test]
    fn parse_snapshot_key_rejects_wrong_extension() {
        assert!(parse_snapshot_key("preprod/69206375.hash.tar.gz", "preprod/").is_none());
    }

    #[test]
    fn parse_snapshot_key_rejects_nested_path() {
        assert!(parse_snapshot_key("preprod/sub/69206375.hash.tar.zst", "preprod/").is_none());
    }

    #[test]
    fn parse_snapshot_key_rejects_non_numeric_slot() {
        assert!(parse_snapshot_key("preprod/noslot.hash.tar.zst", "preprod/").is_none());
    }

    #[test]
    fn parse_snapshot_key_rejects_invalid_hash() {
        assert!(parse_snapshot_key("preprod/69206375.hash.tar.zst", "preprod/").is_none());
        assert!(
            parse_snapshot_key(
                "preprod/69206375.6f99b5f3deaeae8dc43fce3db2f3cd36ad8ed174ca3400b5b1bed76fdf248912.extra.tar.zst",
                "preprod/"
            )
            .is_none()
        );
    }
}
