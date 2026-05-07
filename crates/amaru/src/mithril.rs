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
    fmt, fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use amaru_kernel::NetworkName;
use async_trait::async_trait;
use indicatif::{MultiProgress, ProgressBar, ProgressState, ProgressStyle};
use mithril_client::{
    ClientBuilder, MessageBuilder,
    cardano_database_client::{DownloadUnpackOptions, ImmutableFileRange},
    feedback::{FeedbackReceiver, MithrilEvent, MithrilEventCardanoDatabase},
};
use tokio::sync::RwLock;
use tracing::info;

struct AggregatorDetails {
    endpoint: &'static str,
    verification_key: &'static str,
    ancillary_verification_key: &'static str,
}

struct IndicatifFeedbackReceiver {
    progress_bar: MultiProgress,
    cardano_database_pb: RwLock<Option<ProgressBar>>,
    certificate_validation_pb: RwLock<Option<ProgressBar>>,
}

impl IndicatifFeedbackReceiver {
    fn new(progress_bar: &MultiProgress) -> Self {
        Self {
            progress_bar: progress_bar.clone(),
            cardano_database_pb: RwLock::new(None),
            certificate_validation_pb: RwLock::new(None),
        }
    }
}

#[async_trait]
#[allow(clippy::wildcard_enum_match_arm)]
#[allow(clippy::unwrap_used)]
impl FeedbackReceiver for IndicatifFeedbackReceiver {
    async fn handle_event(&self, event: MithrilEvent) {
        match event {
            MithrilEvent::CardanoDatabase(cardano_database_event) => match cardano_database_event {
                MithrilEventCardanoDatabase::Started { download_id: _, total_immutable_files, include_ancillary } => {
                    let size = match include_ancillary {
                        true => 1 + total_immutable_files,
                        false => total_immutable_files,
                    };
                    let pb = ProgressBar::new(size);
                    pb.set_style(
                        ProgressStyle::with_template(
                            "{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] Files: {human_pos}/{human_len} ({eta})",
                        )
                        .unwrap()
                        .with_key("eta", |state: &ProgressState, w: &mut dyn fmt::Write| {
                            write!(w, "{:.1}s", state.eta().as_secs_f64()).unwrap()
                        })
                        .progress_chars("#>-"),
                    );
                    self.progress_bar.add(pb.clone());
                    let mut cardano_database_pb = self.cardano_database_pb.write().await;
                    *cardano_database_pb = Some(pb);
                }
                MithrilEventCardanoDatabase::Completed { .. } => {
                    let mut cardano_database_pb = self.cardano_database_pb.write().await;
                    if let Some(progress_bar) = cardano_database_pb.as_ref() {
                        progress_bar.finish_with_message("Artifact files download completed");
                    }
                    *cardano_database_pb = None;
                }
                MithrilEventCardanoDatabase::ImmutableDownloadCompleted { .. }
                | MithrilEventCardanoDatabase::AncillaryDownloadCompleted { .. } => {
                    let cardano_database_pb = self.cardano_database_pb.read().await;
                    if let Some(progress_bar) = cardano_database_pb.as_ref() {
                        progress_bar.inc(1);
                    }
                }
                _ => {}
            },
            MithrilEvent::CertificateChainValidationStarted { certificate_chain_validation_id: _ } => {
                let pb = ProgressBar::new_spinner();
                self.progress_bar.add(pb.clone());
                let mut certificate_validation_pb = self.certificate_validation_pb.write().await;
                *certificate_validation_pb = Some(pb);
            }
            MithrilEvent::CertificateValidated { certificate_chain_validation_id: _, certificate_hash } => {
                let certificate_validation_pb = self.certificate_validation_pb.read().await;
                if let Some(progress_bar) = certificate_validation_pb.as_ref() {
                    progress_bar.set_message(format!("Certificate '{certificate_hash}' is valid"));
                    progress_bar.inc(1);
                }
            }
            MithrilEvent::CertificateChainValidated { certificate_chain_validation_id: _ } => {
                let mut certificate_validation_pb = self.certificate_validation_pb.write().await;
                if let Some(progress_bar) = certificate_validation_pb.as_ref() {
                    progress_bar.finish_with_message("Certificate chain validated");
                }
                *certificate_validation_pb = None;
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
            ancillary_verification_key: "5b32332c37312c39362c3133332c34372c3235332c3232362c3133362c3233352c35372c3136342c3130362c3138362c322c32312c32392c3132302c3136332c38392c3132312c3137372c3133382c3230382c3133382c3231342c39392c35382c32322c302c35382c332c36395d",
        }),
        NetworkName::Preprod => Ok(AggregatorDetails {
            endpoint: "https://aggregator.release-preprod.api.mithril.network/aggregator",
            verification_key: "5b3132372c37332c3132342c3136312c362c3133372c3133312c3231332c3230372c3131372c3139382c38352c3137362c3139392c3136322c3234312c36382c3132332c3131392c3134352c31332c3233322c3234332c34392c3232392c322c3234392c3230352c3230352c33392c3233352c34345d",
            ancillary_verification_key: "5b3138392c3139322c3231362c3135302c3131342c3231362c3233372c3231302c34352c31382c32312c3139362c3230382c3234362c3134362c322c3235322c3234332c3235312c3139372c32382c3135372c3230342c3134352c33302c31342c3232382c3136382c3132392c38332c3133362c33365d",
        }),
        NetworkName::Preview => Ok(AggregatorDetails {
            endpoint: "https://aggregator.testing-preview.api.mithril.network/aggregator",
            verification_key: "5b3132372c37332c3132342c3136312c362c3133372c3133312c3231332c3230372c3131372c3139382c38352c3137362c3139392c3136322c3234312c36382c3132332c3131392c3134352c31332c3233322c3234332c34392c3232392c322c3234392c3230352c3230352c33392c3233352c34345d",
            ancillary_verification_key: "5b3138392c3139322c3231362c3135302c3131342c3231362c3233372c3231302c34352c31382c32312c3139362c3230382c3234362c3134362c322c3235322c3234332c3235312c3139372c32382c3135372c3230342c3134352c33302c31342c3232382c3136382c3132392c38332c3133362c33365d",
        }),
        NetworkName::Testnet(_) => Err("Mithril is only supported on mainnet, preprod and preview".into()),
    }
}

pub async fn download_from_mithril(
    network: NetworkName,
    target_dir: PathBuf,
    from_chunk: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let progress_bar = MultiProgress::new();
    let AggregatorDetails { endpoint, verification_key, ancillary_verification_key } = aggregator_details(network)?;
    let client = ClientBuilder::aggregator(endpoint, verification_key)
        .set_ancillary_verification_key(ancillary_verification_key.to_string())
        .with_origin_tag(Some("AMARU".to_string()))
        .add_feedback_receiver(Arc::new(IndicatifFeedbackReceiver::new(&progress_bar)))
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

pub fn get_latest_chunk(immutable_dir: &Path) -> Result<Option<u64>, io::Error> {
    if !immutable_dir.try_exists()? {
        return Ok(None);
    }

    Ok(fs::read_dir(immutable_dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| entry.path().file_name()?.to_str().map(str::to_owned))
        .filter_map(|name| name.strip_suffix(".chunk").and_then(|id| id.parse::<u64>().ok()))
        .max()
        .map(|n| n.saturating_sub(1)))
}
