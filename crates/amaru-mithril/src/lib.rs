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
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::{self, File},
    io::{self, Cursor, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use amaru_kernel::{Hasher, NetworkName, Point, cbor};
use async_trait::async_trait;
use flate2::{Compression, GzBuilder};
use indicatif::{MultiProgress, ProgressBar, ProgressState, ProgressStyle};
use mithril_client::{
    ClientBuilder, MessageBuilder,
    cardano_database_client::{DownloadUnpackOptions, ImmutableFileRange},
    feedback::{FeedbackReceiver, MithrilEvent, MithrilEventCardanoDatabase},
};
use tar::{Builder, Header};
use tokio::sync::RwLock;
use tracing::info;

pub const BLOCKS_PER_ARCHIVE: usize = 20000;

struct AggregatorDetails {
    endpoint: &'static str,
    verification_key: &'static str,
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
) -> Result<(), Box<dyn std::error::Error>> {
    let progress_bar = MultiProgress::new();
    let AggregatorDetails { endpoint, verification_key } = aggregator_details(network)?;
    let client = ClientBuilder::aggregator(endpoint, verification_key)
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

fn block_file_name(point: &Point) -> String {
    format!("{point}.cbor")
}

fn build_archive_bytes(blocks: &BTreeMap<Point, &Vec<u8>>) -> io::Result<Vec<u8>> {
    let encoder = GzBuilder::new().mtime(0).write(Vec::new(), Compression::default());
    let mut tar = Builder::new(encoder);

    for (point, data) in blocks {
        let mut header = Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        header.set_cksum();

        tar.append_data(&mut header, block_file_name(point), Cursor::new(data))?;
    }

    let encoder = tar.into_inner()?;
    encoder.finish()
}

pub fn archive_name_for_blocks(blocks: &BTreeMap<Point, &Vec<u8>>) -> Option<String> {
    let (first_block, _) = blocks.first_key_value()?;
    let (last_block, _) = blocks.last_key_value()?;

    Some(format!("{first_block}__{last_block}.tar.gz"))
}

#[allow(clippy::expect_used)]
pub fn package_blocks(blocks_dir: &Path, blocks: &BTreeMap<Point, &Vec<u8>>) -> io::Result<String> {
    let compressed = build_archive_bytes(blocks)?;

    fs::create_dir_all(blocks_dir)?;
    let archive_name = archive_name_for_blocks(blocks).expect("blocks map is non-empty here by construction");
    let archive_path = blocks_dir.join(&archive_name);
    let archive_path_str = archive_path.to_string_lossy().into_owned();
    let mut file = File::create(&archive_path)?;
    file.write_all(&compressed)?;

    Ok(archive_path_str)
}

pub fn list_existing_archives(blocks_dir: &Path) -> Result<BTreeSet<String>, io::Error> {
    if !blocks_dir.try_exists()? {
        return Ok(BTreeSet::new());
    }

    Ok(fs::read_dir(blocks_dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(".tar.gz"))
        .collect())
}

fn parse_archive_point(name: &str) -> Option<Point> {
    Point::try_from(name).ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveMetadata {
    pub file_name: String,
    pub first_block: Point,
    pub last_block: Point,
}

fn parse_archive_metadata(archive_name: &str) -> Option<ArchiveMetadata> {
    let archive_name = archive_name.strip_suffix(".tar.gz")?;
    let (first_block, last_block) = archive_name.split_once("__")?;

    Some(ArchiveMetadata {
        file_name: format!("{archive_name}.tar.gz"),
        first_block: parse_archive_point(first_block)?,
        last_block: parse_archive_point(last_block)?,
    })
}

#[cfg(test)]
fn parse_archive_bounds(archive_name: &str) -> Option<(Point, Point)> {
    let metadata = parse_archive_metadata(archive_name)?;

    Some((metadata.first_block, metadata.last_block))
}

#[cfg(test)]
fn latest_archive_end_point<'a>(archives: impl IntoIterator<Item = &'a String>) -> Option<Point> {
    archives
        .into_iter()
        .filter_map(|archive_name| parse_archive_metadata(archive_name))
        .map(|archive| archive.last_block)
        .max()
}

fn sorted_archives<'a>(archives: impl IntoIterator<Item = &'a String>) -> Vec<ArchiveMetadata> {
    let mut parsed: Vec<_> =
        archives.into_iter().filter_map(|archive_name| parse_archive_metadata(archive_name)).collect();
    parsed.sort_by_key(|archive| archive.last_block);
    parsed
}

pub fn latest_archive<'a>(archives: impl IntoIterator<Item = &'a String>) -> Option<ArchiveMetadata> {
    sorted_archives(archives).into_iter().last()
}

pub fn resume_point_for_archives<'a>(archives: impl IntoIterator<Item = &'a String>) -> Point {
    let parsed = sorted_archives(archives);

    parsed.iter().rev().nth(1).map(|archive| archive.last_block).unwrap_or(Point::Origin)
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

fn infer_chunk_from_slot(slot: u64) -> u64 {
    slot / 21_600
}

pub fn from_chunk_for_resume_point(latest_chunk: Option<u64>, resume_point: Point) -> u64 {
    latest_chunk.unwrap_or_else(|| infer_chunk_from_slot(resume_point.slot_or_default().into()).saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use amaru_kernel::Point;

    use super::{
        ArchiveMetadata, archive_name_for_blocks, latest_archive, latest_archive_end_point, parse_archive_bounds,
    };

    #[test]
    fn archive_name_includes_first_and_last_blocks() {
        let block_a = Vec::from([0x01_u8]);
        let block_b = Vec::from([0x02_u8]);
        let mut blocks = BTreeMap::new();

        blocks.insert(
            Point::try_from("10.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap(),
            &block_a,
        );
        blocks.insert(
            Point::try_from("20.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap(),
            &block_b,
        );

        assert_eq!(
            archive_name_for_blocks(&blocks),
            Some(
                "10.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa__20.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.tar.gz"
                    .to_string()
            )
        );
    }

    #[test]
    fn archive_name_uses_point_order_across_decimal_boundaries() {
        let block_a = Vec::from([0x01_u8]);
        let block_b = Vec::from([0x02_u8]);
        let mut blocks = BTreeMap::new();

        blocks.insert(
            Point::try_from("100000.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap(),
            &block_a,
        );
        blocks.insert(
            Point::try_from("99999.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap(),
            &block_b,
        );

        assert_eq!(
            archive_name_for_blocks(&blocks),
            Some(
                "99999.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb__100000.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.tar.gz"
                    .to_string()
            )
        );
    }

    #[test]
    fn archive_name_is_absent_for_empty_batch() {
        let blocks: BTreeMap<Point, &Vec<u8>> = BTreeMap::new();

        assert_eq!(archive_name_for_blocks(&blocks), None);
    }

    #[test]
    fn parse_archive_bounds_extracts_first_and_last_points() {
        let bounds = parse_archive_bounds(
            "10.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa__20.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.tar.gz",
        );

        assert_eq!(
            bounds,
            Some((
                Point::try_from("10.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap(),
                Point::try_from("20.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap(),
            ))
        );
    }

    #[test]
    fn latest_archive_end_point_uses_last_block_boundary() {
        let archives = vec![
            "10.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa__20.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.tar.gz".to_string(),
            "21.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc__30.dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd.tar.gz".to_string(),
        ];

        assert_eq!(
            latest_archive_end_point(&archives),
            Some(Point::try_from("30.dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd").unwrap())
        );
    }

    #[test]
    fn latest_archive_picks_last_archive() {
        let archives = vec![
            "10.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa__20.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.tar.gz".to_string(),
            "21.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc__25.dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd.tar.gz".to_string(),
        ];

        assert_eq!(
            latest_archive(&archives),
            Some(ArchiveMetadata {
                file_name: "21.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc__25.dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd.tar.gz".to_string(),
                first_block: Point::try_from("21.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc").unwrap(),
                last_block: Point::try_from("25.dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd").unwrap(),
            })
        );
    }
}
