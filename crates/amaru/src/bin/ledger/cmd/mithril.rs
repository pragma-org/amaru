// Copyright 2025 PRAGMA
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
    collections::BTreeMap,
    fmt,
    fs::{self, File},
    io::{self, Cursor, Write},
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use amaru::point::to_network_point;
use amaru_kernel::{default_ledger_dir, network::NetworkName};
use async_trait::async_trait;
use clap::Parser;
use indicatif::{MultiProgress, ProgressBar, ProgressState, ProgressStyle};
use mithril_client::{
    ClientBuilder, MessageBuilder,
    cardano_database_client::{DownloadUnpackOptions, ImmutableFileRange},
    feedback::{FeedbackReceiver, MithrilEvent, MithrilEventCardanoDatabase},
};
use pallas_hardano::storage::immutable::read_blocks_from_point;
use pallas_traverse::MultiEraBlock;
use tar::Builder;
use tokio::sync::RwLock;
use tracing::info;

use crate::cmd::new_block_validator;

#[derive(Debug, Parser)]
pub struct Args {
    /// The target network to choose from.
    ///
    /// Should be one of 'mainnet', 'preprod', 'preview' or 'testnet:<magic>' where
    /// `magic` is a 32-bits unsigned value denoting a particular testnet.
    #[arg(
        long,
        value_name = "NETWORK_NAME",
        env = "AMARU_NETWORK",
        default_value_t = NetworkName::Preprod,
        verbatim_doc_comment
    )]
    network: NetworkName,

    /// Path of the ledger on-disk storage.
    #[arg(
        long,
        value_name = "DIR",
        env = "AMARU_LEDGER_DIR",
        verbatim_doc_comment
    )]
    ledger_dir: Option<PathBuf>,

    /// Path of the ledger on-disk storage.
    #[arg(long, value_name = "DIR", default_value = Some("mithril-snapshots".into()), env = "AMARU_MITHRIL_SNAPSHOTS_DIR", verbatim_doc_comment)]
    snapshots_dir: PathBuf,
}

pub struct IndicatifFeedbackReceiver {
    progress_bar: MultiProgress,
    cardano_database_pb: RwLock<Option<ProgressBar>>,
    certificate_validation_pb: RwLock<Option<ProgressBar>>,
}

impl IndicatifFeedbackReceiver {
    pub fn new(progress_bar: &MultiProgress) -> Self {
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
                MithrilEventCardanoDatabase::Started {
                    download_id: _,
                    total_immutable_files,
                    include_ancillary,
                } => {
                    let size = match include_ancillary {
                        true => 1 + total_immutable_files,
                        false => total_immutable_files,
                    };
                    let pb = ProgressBar::new(size);
                    pb.set_style(ProgressStyle::with_template("{spinner:.green} {elapsed_precise}] [{wide_bar:.cyan/blue}] Files: {human_pos}/{human_len} ({eta})")
                        .unwrap()
                        .with_key("eta", |state: &ProgressState, w: &mut dyn fmt::Write| write!(w, "{:.1}s", state.eta().as_secs_f64()).unwrap())
                        .progress_chars("#>-"));
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
                _ => {
                    // Ignore other events
                }
            },
            MithrilEvent::CertificateChainValidationStarted {
                certificate_chain_validation_id: _,
            } => {
                let pb = ProgressBar::new_spinner();
                self.progress_bar.add(pb.clone());
                let mut certificate_validation_pb = self.certificate_validation_pb.write().await;
                *certificate_validation_pb = Some(pb);
            }
            MithrilEvent::CertificateValidated {
                certificate_chain_validation_id: _,
                certificate_hash,
            } => {
                let certificate_validation_pb = self.certificate_validation_pb.read().await;
                if let Some(progress_bar) = certificate_validation_pb.as_ref() {
                    progress_bar.set_message(format!("Certificate '{certificate_hash}' is valid"));
                    progress_bar.inc(1);
                }
            }
            MithrilEvent::CertificateChainValidated {
                certificate_chain_validation_id: _,
            } => {
                let mut certificate_validation_pb = self.certificate_validation_pb.write().await;
                if let Some(progress_bar) = certificate_validation_pb.as_ref() {
                    progress_bar.finish_with_message("Certificate chain validated");
                }
                *certificate_validation_pb = None;
            }
            _ => {
                // Ignore other events
            }
        }
    }
}

#[allow(clippy::unwrap_used)]
async fn package_blocks(
    network: &NetworkName,
    blocks: &BTreeMap<String, &Vec<u8>>,
) -> std::io::Result<Vec<u8>> {
    use flate2::{Compression, write::GzEncoder};
    use tar::Header;

    // Create a GzEncoder that will write compressed bytes to an in-memory Vec<u8>
    let encoder = GzEncoder::new(Vec::new(), Compression::default());

    // Give ownership of the encoder to the tar builder so it can write into it
    let mut tar = Builder::new(encoder);

    for (name, data) in blocks {
        let mut header = Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mtime(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        );
        header.set_uid(0);
        header.set_gid(0);
        header.set_cksum();

        tar.append_data(&mut header, name, Cursor::new(data))?;
    }

    // Extract the inner encoder (GzEncoder<Vec<u8>>)
    let encoder = tar.into_inner()?;

    // Finish compression and obtain the inner Vec<u8>
    let compressed = encoder.finish()?;

    let dir = format!("data/{}/blocks", network);
    fs::create_dir_all(&dir)?;
    let first_block = blocks
        .first_key_value()
        .map(|kv| kv.0)
        .cloned()
        .unwrap_or_default();
    let archive_path = format!("{}/{}.tar.gz", dir, first_block);
    let mut f = File::create(archive_path)?;

    f.write_all(&compressed)?;

    Ok(compressed)
}

/// Returns the latest chunk number present in the given immutable directory.
/// If no chunk files are found, returns None.
fn ger_latest_chunk(immutable_dir: &PathBuf) -> Result<Option<u64>, io::Error> {
    if immutable_dir.try_exists()? {
        return Ok(fs::read_dir(immutable_dir)?
            .filter_map(Result::ok)
            .filter_map(|entry| entry.path().file_name()?.to_str().map(|s| s.to_owned()))
            .filter_map(|name| {
                name.strip_suffix(".chunk")
                    .and_then(|id| id.parse::<u64>().ok())
            })
            .max()
            .map(|n| n - 1));
    }
    Ok(None)
}

struct AggregatorDetails {
    endpoint: &'static str,
    verification_key: &'static str,
    ancillary_verification_key: &'static str,
    initial_chunk: u64,
}

// See https://github.com/input-output-hk/mithril/blob/main/networks.json
#[allow(clippy::panic)]
fn aggregator_details(network: NetworkName) -> AggregatorDetails {
    match network {
        NetworkName::Mainnet => AggregatorDetails {
            endpoint: "https://aggregator.release-mainnet.api.mithril.network/aggregator",
            verification_key: "5b3139312c36362c3134302c3138352c3133382c31312c3233372c3230372c3235302c3134342c32372c322c3138382c33302c31322c38312c3135352c3230342c31302c3137392c37352c32332c3133382c3139362c3231372c352c31342c32302c35372c37392c33392c3137365d",
            ancillary_verification_key: "5b32332c37312c39362c3133332c34372c3235332c3232362c3133362c3233352c35372c3136342c3130362c3138362c322c32312c32392c3132302c3136332c38392c3132312c3137372c3133382c3230382c3133382c3231342c39392c35382c32322c302c35382c332c36395d",
            initial_chunk: 4500,
        },
        NetworkName::Preprod => AggregatorDetails {
            endpoint: "https://aggregator.release-preprod.api.mithril.network/aggregator",
            verification_key: "5b3132372c37332c3132342c3136312c362c3133372c3133312c3231332c3230372c3131372c3139382c38352c3137362c3139392c3136322c3234312c36382c3132332c3131392c3134352c31332c3233322c3234332c34392c3232392c322c3234392c3230352c3230352c33392c3233352c34345d",
            ancillary_verification_key: "5b3138392c3139322c3231362c3135302c3131342c3231362c3233372c3231302c34352c31382c32312c3139362c3230382c3234362c3134362c322c3235322c3234332c3235312c3139372c32382c3135372c3230342c3134352c33302c31342c3232382c3136382c3132392c38332c3133362c33365d",
            initial_chunk: 4500,
        },
        NetworkName::Preview => AggregatorDetails {
            endpoint: "https://aggregator.testing-preview.api.mithril.network/aggregator",
            verification_key: "5b3132372c37332c3132342c3136312c362c3133372c3133312c3231332c3230372c3131372c3139382c38352c3137362c3139392c3136322c3234312c36382c3132332c3131392c3134352c31332c3233322c3234332c34392c3232392c322c3234392c3230352c3230352c33392c3233352c34345d",
            ancillary_verification_key: "5b3138392c3139322c3231362c3135302c3131342c3231362c3233372c3231302c34352c31382c32312c3139362c3230382c3234362c3134362c322c3235322c3234332c3235312c3139372c32382c3135372c3230342c3134352c33302c31342c3232382c3136382c3132392c38332c3133362c33365d",
            initial_chunk: 4500,
        },
        NetworkName::Testnet(_) => panic!("Mithril not supported on testnets"),
    }
}

async fn download_from_mithril(
    network: NetworkName,
    target_dir: PathBuf,
    latest_chunk: Option<u64>,
) -> Result<(), Box<dyn std::error::Error>> {
    let progress_bar = indicatif::MultiProgress::new();
    let AggregatorDetails {
        endpoint,
        verification_key,
        ancillary_verification_key,
        initial_chunk,
    } = aggregator_details(network);
    let client = ClientBuilder::aggregator(endpoint, verification_key)
        .set_ancillary_verification_key(ancillary_verification_key.to_string())
        .add_feedback_receiver(Arc::new(IndicatifFeedbackReceiver::new(&progress_bar)))
        .build()?;
    let database_client = client.cardano_database_v2();
    let snapshots = database_client.list().await?;
    let snapshot_list_item = snapshots.first().ok_or("no snapshot found")?;
    let snapshot = database_client
        .get(&snapshot_list_item.hash)
        .await?
        .ok_or(format!("snapshot not found {}", snapshot_list_item.hash))?;
    let certificate = client
        .certificate()
        .verify_chain(&snapshot.certificate_hash)
        .await?;
    let from_chunk = latest_chunk.unwrap_or(initial_chunk);
    info!("Downloading mithril immutabe starting chunk {}", from_chunk);
    let immutable_file_range = ImmutableFileRange::From(from_chunk);
    let download_unpack_options = DownloadUnpackOptions {
        allow_override: true,
        include_ancillary: false,
        ..DownloadUnpackOptions::default()
    };
    database_client
        .download_unpack(
            &snapshot,
            &immutable_file_range,
            &target_dir,
            download_unpack_options,
        )
        .await?;
    info!("Snapshot unpacked to {:?}", target_dir);
    let verified_digests = client
        .cardano_database_v2()
        .download_and_verify_digests(&certificate, &snapshot)
        .await?;
    let allow_missing_immutables_files = false;
    let merkle_proof = client
        .cardano_database_v2()
        .verify_cardano_database(
            &certificate,
            &snapshot,
            &immutable_file_range,
            allow_missing_immutables_files,
            &target_dir,
            &verified_digests,
        )
        .await?;
    let message = MessageBuilder::new()
        .compute_cardano_database_message(&certificate, &merkle_proof)
        .await?;
    assert!(certificate.match_message(&message));
    info!("Snapshot verified against certificate");
    Ok(())
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let network = args.network;
    let ledger_dir = args
        .ledger_dir
        .unwrap_or_else(|| default_ledger_dir(network).into());

    let target_dir = args.snapshots_dir.join(network.to_string());
    fs::create_dir_all(&target_dir)?;

    let immutable_dir = target_dir.join("immutable");

    let ledger = new_block_validator(network, ledger_dir)?;
    let tip = ledger.get_tip();

    let latest_chunk = ger_latest_chunk(&immutable_dir)?;

    download_from_mithril(network, target_dir, latest_chunk).await?;

    for chunk in read_blocks_from_point(&immutable_dir, to_network_point(tip))?
        .map_while(Result::ok)
        .skip(1) // Exclude the tip itself
        .array_chunks::<20000>()
    {
        let map: BTreeMap<_, _> = chunk
            .iter()
            .filter_map(|cbor| {
                let block = MultiEraBlock::decode(cbor).ok()?;
                let header = block.header();
                let name = format!("{}.{}.cbor", header.slot(), header.hash());
                Some((name, cbor))
            })
            .collect();
        package_blocks(&network, &map).await?;
    }

    Ok(())
}
