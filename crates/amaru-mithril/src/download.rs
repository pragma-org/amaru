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

use std::{path::PathBuf, sync::Arc};

use amaru_kernel::NetworkName;
use amaru_progress_bar::ProgressBar;
use async_trait::async_trait;
use mithril_client::{
    ClientBuilder, GenesisVerificationKey, MessageBuilder,
    cardano_database_client::{DownloadUnpackOptions, ImmutableFileRange},
    feedback::{FeedbackReceiver, MithrilEvent, MithrilEventCardanoDatabase},
};
use tokio::sync::Mutex;
use tracing::info;

type ProgressFactory = Arc<dyn Fn(usize, &str) -> Box<dyn ProgressBar + Send + Sync> + Send + Sync>;

struct AggregatorDetails {
    endpoint: &'static str,
    verification_key: &'static str,
}

struct MithrilFeedbackReceiver {
    with_progress: ProgressFactory,
    cardano_database_pb: Mutex<Option<Box<dyn ProgressBar + Send + Sync>>>,
    certificate_validation_pb: Mutex<Option<Box<dyn ProgressBar + Send + Sync>>>,
}

impl MithrilFeedbackReceiver {
    fn new(with_progress: ProgressFactory) -> Self {
        Self { with_progress, cardano_database_pb: Mutex::new(None), certificate_validation_pb: Mutex::new(None) }
    }
}

#[async_trait]
#[allow(clippy::wildcard_enum_match_arm)]
impl FeedbackReceiver for MithrilFeedbackReceiver {
    async fn handle_event(&self, event: MithrilEvent) {
        match event {
            MithrilEvent::CardanoDatabase(cardano_database_event) => match cardano_database_event {
                MithrilEventCardanoDatabase::Started { download_id: _, total_immutable_files, include_ancillary } => {
                    let size = match include_ancillary {
                        true => 1 + total_immutable_files,
                        false => total_immutable_files,
                    };
                    let pb = (self.with_progress)(
                        size as usize,
                        "{spinner:.green} {elapsed_precise} [{bar:40.green}] {pos}/{len} files (eta {eta})",
                    );
                    *self.cardano_database_pb.lock().await = Some(pb);
                }
                MithrilEventCardanoDatabase::Completed { .. } => {
                    if let Some(pb) = self.cardano_database_pb.lock().await.take() {
                        pb.clear();
                    }
                }
                MithrilEventCardanoDatabase::ImmutableDownloadCompleted { .. }
                | MithrilEventCardanoDatabase::AncillaryDownloadCompleted { .. } => {
                    if let Some(pb) = self.cardano_database_pb.lock().await.as_ref() {
                        pb.tick(1);
                    }
                }
                _ => {}
            },
            MithrilEvent::CertificateChainValidationStarted { .. } => {
                let pb = (self.with_progress)(0, "{spinner:.green} {elapsed_precise} {msg}");
                *self.certificate_validation_pb.lock().await = Some(pb);
            }
            MithrilEvent::CertificateValidated { certificate_chain_validation_id: _, certificate_hash } => {
                if let Some(pb) = self.certificate_validation_pb.lock().await.as_ref() {
                    let _ = certificate_hash;
                    pb.tick(0);
                }
            }
            MithrilEvent::CertificateChainValidated { .. } => {
                if let Some(pb) = self.certificate_validation_pb.lock().await.take() {
                    pb.clear();
                }
            }
            _ => {}
        }
    }
}

fn aggregator_details(network: NetworkName) -> Result<AggregatorDetails, Box<dyn std::error::Error>> {
    match network {
        NetworkName::Mainnet => Ok(AggregatorDetails {
            endpoint: "https://aggregator.release-mainnet.api.mithril.network/aggregator",
            verification_key: "5b3139312c36362c3134302c3138352c3133382c31312c3233372c3230372c3235302c3134342c32372c322c3138382c33302c31322c38312c3135352c3230342c31302c3137392c37352c32332c3133382c3139362c3231372c352c31342c32302c35372c37392c33392c3137365d",
        }),
        NetworkName::Preprod => Ok(AggregatorDetails {
            endpoint: "https://aggregator.release-preprod.api.mithril.network/aggregator",
            verification_key: "5b3132372c37332c3132342c3136312c362c3133372c3133312c3231332c3230372c3131372c3139382c38352c3137362c3139392c3136322c3234312c36382c3132332c3131392c3134352c31332c3233322c3234332c34392c3232392c322c3234392c3230352c3230352c33392c3233352c34345d",
        }),
        NetworkName::Preview => Ok(AggregatorDetails {
            endpoint: "https://aggregator.testing-preview.api.mithril.network/aggregator",
            verification_key: "5b3132372c37332c3132342c3136312c362c3133372c3133312c3231332c3230372c3131372c3139382c38352c3137362c3139392c3136322c3234312c36382c3132332c3131392c3134352c31332c3233322c3234332c34392c3232392c322c3234392c3230352c3230352c33392c3233352c34345d",
        }),
        NetworkName::Testnet(_) => Err("Mithril is only supported on mainnet, preprod and preview".into()),
    }
}

pub async fn download_from_mithril(
    network: NetworkName,
    target_dir: PathBuf,
    from_chunk: u64,
    with_progress: Arc<dyn Fn(usize, &str) -> Box<dyn ProgressBar + Send + Sync> + Send + Sync>,
) -> Result<(), Box<dyn std::error::Error>> {
    let AggregatorDetails { endpoint, verification_key } = aggregator_details(network)?;
    let client = ClientBuilder::new(mithril_client::AggregatorDiscoveryType::Url(endpoint.to_string()))
        .set_genesis_verification_key(GenesisVerificationKey::JsonHex(verification_key.into()))
        .with_origin_tag(Some("AMARU".to_string()))
        .add_feedback_receiver(Arc::new(MithrilFeedbackReceiver::new(with_progress)))
        .build()?;
    let database_client = client.cardano_database_v2();
    let snapshots = database_client.list().await?;
    let snapshot_list_item = snapshots.first().ok_or("no Mithril cardano-db snapshot found")?;

    info!(hash = %snapshot_list_item.hash, from_chunk, "downloading and verifying Mithril snapshot");

    let snapshot = database_client
        .get(&snapshot_list_item.hash)
        .await?
        .ok_or_else(|| format!("Mithril snapshot not found: {}", snapshot_list_item.hash))?;
    let certificate = client.certificate().verify_chain(&snapshot.certificate_hash).await?;

    let immutable_file_range = ImmutableFileRange::From(from_chunk);
    let download_unpack_options =
        DownloadUnpackOptions { allow_override: true, include_ancillary: false, ..DownloadUnpackOptions::default() };
    info!(target_dir = %target_dir.display(), from_chunk, "certificate chain validated; downloading and unpacking immutable files");
    database_client.download_unpack(&snapshot, &immutable_file_range, &target_dir, download_unpack_options).await?;

    info!(target_dir = %target_dir.display(), "immutable files unpacked; downloading and verifying Mithril digests");
    let verified_digests = client.cardano_database_v2().download_and_verify_digests(&certificate, &snapshot).await?;
    info!(target_dir = %target_dir.display(), "Mithril digests verified; validating local cardano-db against certificate");
    let merkle_proof = client
        .cardano_database_v2()
        .verify_cardano_database(&certificate, &snapshot, &immutable_file_range, false, &target_dir, &verified_digests)
        .await?;
    let message = MessageBuilder::new().compute_cardano_database_message(&certificate, &merkle_proof).await?;

    if !certificate.match_message(&message) {
        return Err("Mithril certificate verification failed".into());
    }

    info!(target_dir = %target_dir.display(), "Mithril cardano-db is ready");

    Ok(())
}
