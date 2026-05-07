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
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{self, Cursor, Write},
    path::PathBuf,
};

use amaru::{
    default_data_dir, default_ledger_dir,
    mithril::{download_from_mithril, get_latest_chunk},
};
use amaru_kernel::{Hasher, NetworkName, Point, cbor};
use amaru_network::point::to_network_point;
use clap::Parser;
use flate2::{Compression, GzBuilder};
use pallas_hardano::storage::immutable::read_blocks_from_point;
use tar::{Builder, Header};
use tracing::{info, warn};

use crate::cmd::new_block_validator;

const BLOCKS_PER_ARCHIVE: usize = 20000;

#[derive(Debug, Parser)]
pub struct Args {
    /// The target network to choose from.
    ///
    /// Should be one of 'mainnet', 'preprod', 'preview' or `testnet:<magic>` where
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
    #[arg(long, value_name = "DIR", env = "AMARU_LEDGER_DIR", verbatim_doc_comment)]
    ledger_dir: Option<PathBuf>,

    /// Path of the mithril snapshots on-disk storage.
    #[arg(
        long,
        value_name = "DIR",
        default_value = "mithril-snapshots",
        env = "AMARU_MITHRIL_SNAPSHOTS_DIR",
        verbatim_doc_comment
    )]
    snapshots_dir: PathBuf,
}

#[allow(clippy::expect_used)]
fn package_blocks(network: &NetworkName, blocks: &BTreeMap<Point, &Vec<u8>>) -> io::Result<String> {
    let compressed = build_archive_bytes(blocks)?;

    let dir = blocks_dir(*network);
    fs::create_dir_all(&dir)?;
    let archive_path = archive_path_for_blocks(network, blocks).expect("blocks map is non-empty here by construction");
    let mut file = File::create(&archive_path)?;
    file.write_all(&compressed)?;

    Ok(archive_path)
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

fn blocks_dir(network: NetworkName) -> String {
    format!("{}/blocks", default_data_dir(network))
}

fn archive_name_for_blocks(blocks: &BTreeMap<Point, &Vec<u8>>) -> Option<String> {
    let (first_block, _) = blocks.first_key_value()?;
    let (last_block, _) = blocks.last_key_value()?;

    Some(format!("{first_block}__{last_block}.tar.gz"))
}

fn archive_path_for_blocks(network: &NetworkName, blocks: &BTreeMap<Point, &Vec<u8>>) -> Option<String> {
    archive_name_for_blocks(blocks).map(|archive_name| format!("{}/{}", blocks_dir(*network), archive_name))
}

fn list_existing_archives(network: NetworkName) -> Result<BTreeSet<String>, io::Error> {
    let dir = PathBuf::from(blocks_dir(network));
    if !dir.try_exists()? {
        return Ok(BTreeSet::new());
    }

    Ok(fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(".tar.gz"))
        .collect())
}

fn parse_archive_point(name: &str) -> Option<Point> {
    Point::try_from(name).ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArchiveMetadata {
    file_name: String,
    first_block: Point,
    last_block: Point,
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

fn latest_archive<'a>(archives: impl IntoIterator<Item = &'a String>) -> Option<ArchiveMetadata> {
    sorted_archives(archives).into_iter().last()
}

fn resume_point_for_archives<'a>(archives: impl IntoIterator<Item = &'a String>) -> Point {
    let parsed = sorted_archives(archives);

    parsed.iter().rev().nth(1).map(|archive| archive.last_block).unwrap_or(Point::Origin)
}

fn infer_chunk_from_slot(slot: u64) -> u64 {
    slot / 21_600
}

fn from_chunk_for_resume_point(latest_chunk: Option<u64>, resume_point: Point) -> u64 {
    latest_chunk.unwrap_or_else(|| infer_chunk_from_slot(resume_point.slot_or_default().into()).saturating_sub(1))
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

pub fn parse_header_slot_and_hash(input: &[u8]) -> Result<ParsedHeader, cbor::decode::Error> {
    let mut dec: cbor::Decoder<'_> = cbor::Decoder::new(input);

    dec.array()?;
    dec.u8()?;
    dec.array()?;
    let header_body_cbor = extract_raw_cbor_value(&mut dec, input)?;

    let header_hash = *Hasher::<256>::hash(header_body_cbor);
    let mut body = cbor::Decoder::new(header_body_cbor);

    body.array()?;
    body.array()?;
    body.u64()?;
    let slot = body.u64()?;
    Ok(ParsedHeader { slot, header_hash })
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let network = args.network;
    let ledger_dir = args.ledger_dir.unwrap_or_else(|| default_ledger_dir(network).into());

    let target_dir = args.snapshots_dir.join(network.to_string());
    fs::create_dir_all(&target_dir)?;

    let immutable_dir = target_dir.join("immutable");

    let ledger = new_block_validator(network, ledger_dir)?;
    let tip = ledger.get_tip();
    let mut existing_archives = list_existing_archives(network)?;
    let tail_archive = latest_archive(&existing_archives);

    // Determine the chunk to start from
    let resume_point = resume_point_for_archives(&existing_archives);

    let latest_chunk = get_latest_chunk(&immutable_dir)?;
    let from_chunk = from_chunk_for_resume_point(latest_chunk, resume_point);

    info!(
        tip=%tip, resume_point=%resume_point, from_chunk=%from_chunk,
        "Downloading mithril immutable chunks"
    );

    download_from_mithril(network, target_dir, from_chunk).await?;

    info!("Packaging blocks into .tar.gz files");

    // Read blocks from the immutable storage and package them into .tar.gz files.
    let mut iter =
        read_blocks_from_point(&immutable_dir, to_network_point(resume_point))?.map_while(Result::ok).skip(1); // Exclude the resume point itself
    loop {
        let chunk: Vec<_> = iter.by_ref().take(BLOCKS_PER_ARCHIVE).collect();
        if chunk.is_empty() {
            break;
        }
        let map: BTreeMap<Point, &Vec<u8>> = chunk
            .iter()
            .map(|cbor| {
                let parsed = parse_header_slot_and_hash(cbor)?;
                let point = Point::Specific(parsed.slot.into(), parsed.header_hash.into());
                Ok((point, cbor))
            })
            .collect::<Result<BTreeMap<Point, &Vec<u8>>, cbor::decode::Error>>()?;

        #[allow(clippy::expect_used)]
        let archive_name = archive_name_for_blocks(&map).expect("chunk map is non-empty here by construction");
        if let Some(tail_archive) = &tail_archive {
            let first_block = map.first_key_value().map(|(point, _)| point);

            if first_block == Some(&tail_archive.first_block) && archive_name == tail_archive.file_name {
                info!(archive = %archive_name, "Retaining existing tail archive");
                continue;
            }

            if first_block == Some(&tail_archive.first_block) && archive_name != tail_archive.file_name {
                let stale_archive_path = PathBuf::from(blocks_dir(network)).join(&tail_archive.file_name);
                fs::remove_file(&stale_archive_path)?;
                existing_archives.remove(&tail_archive.file_name);
                info!(archive = %tail_archive.file_name, replacement = %archive_name, "Replacing tail archive");
            }
        }
        if existing_archives.contains(&archive_name) {
            debug_assert!(false, "encountered an already archived non-tail batch: {}", archive_name);
            warn!(archive = %archive_name, "Encountered an already archived non-tail batch");
            continue;
        }

        let dir = package_blocks(&network, &map)?;
        existing_archives.insert(archive_name);

        info!(blocks = map.len(), dir, "Created archive batch");
    }

    info!("Done");

    Ok(())
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
