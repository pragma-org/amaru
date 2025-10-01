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
    fmt::Write,
    fs,
    io::Cursor,
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use amaru_kernel::network::NetworkName;
use async_trait::async_trait;
use clap::Parser;
use indicatif::{MultiProgress, ProgressBar, ProgressState, ProgressStyle};
use mithril_client::{
    ClientBuilder, MessageBuilder,
    cardano_database_client::{DownloadUnpackOptions, ImmutableFileRange},
    feedback::{FeedbackReceiver, MithrilEvent, MithrilEventCardanoDatabase},
};
use pallas_hardano::storage::immutable::{Point, read_blocks_from_point};
use pallas_traverse::MultiEraBlock;
use tar::Builder;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use tracing::info;

#[derive(Debug, Parser)]
pub struct Args {
    /// The target network to choose from.
    ///
    /// Should be one of 'mainnet', 'preprod', 'preview' or 'testnet:<magic>' where
    /// `magic` is a 32-bits unsigned value denoting a particular testnet.
    #[arg(
        long,
        value_name = "NETWORK",
        default_value_t = NetworkName::Preprod,
    )]
    network: NetworkName,
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
                        .with_key("eta", |state: &ProgressState, w: &mut dyn Write| write!(w, "{:.1}s", state.eta().as_secs_f64()).unwrap())
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
        // Set current mtime (seconds since UNIX epoch)
        header.set_cksum();

        // Append the data (Cursor implements Read)
        tar.append_data(&mut header, name, Cursor::new(data))?;
    }

    // Extract the inner encoder (GzEncoder<Vec<u8>>)
    let encoder = tar.into_inner()?;

    // Finish compression and obtain the inner Vec<u8>
    let compressed = encoder.finish()?;

    let dir = format!("data/{}/blocks", network);
    tokio::fs::create_dir_all(&dir).await?;
    let first_block = blocks
        .first_key_value()
        .map(|kv| kv.0)
        .cloned()
        .unwrap_or_default();
    let archive_path = format!("{}/{}.tar.gz", dir, first_block);
    let mut f = File::create(archive_path).await?;

    f.write_all(&compressed).await?;

    Ok(compressed)
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let network = args.network;

    const AGGREGATOR_ENDPOINT: &str =
        "https://aggregator.release-preprod.api.mithril.network/aggregator";
    const GENESIS_VERIFICATION_KEY: &str = "5b3132372c37332c3132342c3136312c362c3133372c3133312c3231332c3230372c3131372c3139382c38352c3137362c3139392c3136322c3234312c36382c3132332c3131392c3134352c31332c3233322c3234332c34392c3232392c322c3234392c3230352c3230352c33392c3233352c34345d";
    const ANCILLARY_VERIFICATION_KEY: &str = "5b3138392c3139322c3231362c3135302c3131342c3231362c3233372c3231302c34352c31382c32312c3139362c3230382c3234362c3134362c322c3235322c3234332c3235312c3139372c32382c3135372c3230342c3134352c33302c31342c3232382c3136382c3132392c38332c3133362c33365d";
    let progress_bar = indicatif::MultiProgress::new();
    let client = ClientBuilder::aggregator(AGGREGATOR_ENDPOINT, GENESIS_VERIFICATION_KEY)
        .set_ancillary_verification_key(ANCILLARY_VERIFICATION_KEY.to_string())
        .add_feedback_receiver(Arc::new(IndicatifFeedbackReceiver::new(&progress_bar)))
        .build()?;

    let database_client = client.cardano_database_v2();
    let snapshots = database_client.list().await?;
    let snapshot_list_item = snapshots.first().ok_or("no snapshot found")?;
    let snapshot = database_client
        .get(&snapshot_list_item.hash)
        .await?
        .ok_or("no snapshot found")?;

    let certificate = client
        .certificate()
        .verify_chain(&snapshot.certificate_hash)
        .await?;

    let immutable_file_range = ImmutableFileRange::From(1500);
    let download_unpack_options = DownloadUnpackOptions {
        allow_override: true,
        include_ancillary: false,
        ..DownloadUnpackOptions::default()
    };

    let target_dir = PathBuf::from("mithril-snapshots");
    fs::create_dir_all(&target_dir)?;
    database_client
        .download_unpack(
            &snapshot,
            &immutable_file_range,
            &target_dir,
            download_unpack_options,
        )
        .await?;

    info!("Snapshot unpacked to: {:?}", target_dir);

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

    let immutable_dir = target_dir.join("immutable");

    for chunk in read_blocks_from_point(&immutable_dir, Point::new(103444000, vec![]))?
        .map_while(Result::ok)
        .array_chunks::<1000>()
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
