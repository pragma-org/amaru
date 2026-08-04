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
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use amaru_kernel::{GlobalParameters, Hasher, NetworkName, Point, cbor};
use amaru_progress_bar::ProgressBar;
use async_trait::async_trait;
use mithril_client::{
    ClientBuilder, GenesisVerificationKey, MessageBuilder,
    cardano_database_client::{DownloadUnpackOptions, ImmutableFileRange},
    feedback::{FeedbackReceiver, MithrilEvent, MithrilEventCardanoDatabase},
};
use tokio::sync::RwLock;
use tracing::{debug, info};

struct AggregatorDetails {
    endpoint: &'static str,
    verification_key: &'static str,
}

struct ProgressFeedbackReceiver<F> {
    with_progress: F,
    cardano_database: RwLock<Option<Box<dyn ProgressBar>>>,
}

impl<F> ProgressFeedbackReceiver<F> {
    fn new(with_progress: F) -> Self {
        Self { with_progress, cardano_database: RwLock::new(None) }
    }
}

#[async_trait]
#[allow(clippy::wildcard_enum_match_arm)]
impl<F> FeedbackReceiver for ProgressFeedbackReceiver<F>
where
    F: Fn(usize, &str) -> Box<dyn ProgressBar> + Send + Sync,
{
    async fn handle_event(&self, event: MithrilEvent) {
        match event {
            MithrilEvent::CardanoDatabase(cardano_database_event) => match cardano_database_event {
                MithrilEventCardanoDatabase::Started { download_id: _, total_immutable_files, include_ancillary } => {
                    let length = match include_ancillary {
                        true => 1 + total_immutable_files,
                        false => total_immutable_files,
                    };
                    let progress = (self.with_progress)(
                        length as usize,
                        "Downloading Mithril files [{pos}/{len}] {bar:40.green} ({eta} remaining)",
                    );
                    *self.cardano_database.write().await = Some(progress);
                }
                MithrilEventCardanoDatabase::Completed { .. } => {
                    if let Some(progress) = self.cardano_database.write().await.take() {
                        progress.clear();
                    }
                    info!("Mithril immutable files downloaded");
                }
                MithrilEventCardanoDatabase::ImmutableDownloadCompleted { .. }
                | MithrilEventCardanoDatabase::AncillaryDownloadCompleted { .. } => {
                    if let Some(progress) = self.cardano_database.read().await.as_ref() {
                        progress.tick(1);
                    }
                }
                _ => {}
            },
            MithrilEvent::CertificateChainValidationStarted { certificate_chain_validation_id: _ } => {
                info!("Validating Mithril certificate chain");
            }
            MithrilEvent::CertificateValidated { certificate_chain_validation_id: _, certificate_hash } => {
                debug!(%certificate_hash, "Mithril certificate validated");
            }
            MithrilEvent::CertificateChainValidated { certificate_chain_validation_id: _ } => {
                info!("Mithril certificate chain validated");
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

pub async fn download_from_mithril<F>(
    network: NetworkName,
    target_dir: PathBuf,
    from_chunk: u64,
    with_progress: F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: Fn(usize, &str) -> Box<dyn ProgressBar> + Send + Sync + 'static,
{
    let AggregatorDetails { endpoint, verification_key } = aggregator_details(network)?;
    let client = ClientBuilder::new(mithril_client::AggregatorDiscoveryType::Url(endpoint.to_string()))
        .set_genesis_verification_key(GenesisVerificationKey::JsonHex(verification_key.into()))
        .with_origin_tag(Some("AMARU".to_string()))
        .add_feedback_receiver(Arc::new(ProgressFeedbackReceiver::new(with_progress)))
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

#[derive(Debug)]
pub struct ParsedHeader {
    pub slot: u64,
    pub header_hash: [u8; 32],
}

fn extract_raw_cbor_value<'a>(dec: &mut cbor::Decoder<'a>, input: &'a [u8]) -> Result<&'a [u8], cbor::decode::Error> {
    let start = dec.position();
    dec.skip()?;
    let end = dec.position();
    Ok(&input[start..end])
}

pub fn extract_block_header_cbor(input: &[u8]) -> Result<&[u8], cbor::decode::Error> {
    let mut dec: cbor::Decoder<'_> = cbor::Decoder::new(input);

    dec.array()?;
    dec.u8()?;
    dec.array()?;
    extract_raw_cbor_value(&mut dec, input)
}

pub fn parse_header_slot_and_hash(input: &[u8]) -> Result<ParsedHeader, cbor::decode::Error> {
    let header_body_cbor = extract_block_header_cbor(input)?;

    let header_hash = *Hasher::<256>::hash(header_body_cbor);
    let mut body = cbor::Decoder::new(header_body_cbor);

    body.array()?;
    body.array()?;
    body.u64()?;
    let slot = body.u64()?;
    Ok(ParsedHeader { slot, header_hash })
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

pub fn first_missing_immutable_chunk(immutable_dir: &Path) -> Result<u64, io::Error> {
    if !immutable_dir.try_exists()? {
        return Ok(0);
    }

    let mut chunk = 0_u64;
    loop {
        let chunk_prefix = format!("{chunk:05}");
        for extension in ["chunk", "primary", "secondary"] {
            let path = immutable_dir.join(format!("{chunk_prefix}.{extension}"));
            match fs::metadata(&path) {
                Ok(metadata) if metadata.is_file() => {}
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("expected a regular file at {}", path.display()),
                    ));
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(chunk),
                Err(err) => return Err(err),
            }
        }
        chunk = chunk.checked_add(1).ok_or_else(|| io::Error::other("immutable chunk index overflows u64"))?;
    }
}

pub fn chunk_for_slot(network: NetworkName, slot: u64) -> anyhow::Result<u64> {
    // Immutable chunks span one Byron epoch, i.e. 10 * k slots: 21600 on
    // mainnet and preprod (k = 2160), 4320 on preview (k = 432).
    let global_parameters: &GlobalParameters = network
        .as_global_parameters()
        .ok_or_else(|| anyhow::anyhow!("GlobalParameters not know for network name `{}`", network))?;
    let slots_per_chunk = 10 * global_parameters.consensus_security_param;
    Ok(slot / slots_per_chunk)
}

pub fn from_chunk_for_resume_point(
    network: NetworkName,
    latest_chunk: Option<u64>,
    resume_point: Point,
) -> anyhow::Result<u64> {
    if let Some(latest) = latest_chunk {
        return Ok(latest);
    }
    Ok(chunk_for_slot(network, resume_point.slot_or_default().into())?.saturating_sub(1))
}
