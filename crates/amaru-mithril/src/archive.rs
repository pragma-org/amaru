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

// Utilities for packaging raw cardano-node blocks (read from the immutable store) into
// batched `.tar.gz` archives for use by the `amaru-ledger mithril` / `amaru-ledger sync`
// pipeline.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{self, Cursor, Write},
    path::Path,
};

use amaru_kernel::Point;
use flate2::{Compression, GzBuilder};
use tar::{Builder, Header};

pub const BLOCKS_PER_ARCHIVE: usize = 20000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveMetadata {
    pub file_name: String,
    pub first_block: Point,
    pub last_block: Point,
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

pub fn parse_archive_metadata(archive_name: &str) -> Option<ArchiveMetadata> {
    let archive_name = archive_name.strip_suffix(".tar.gz")?;
    let (first_block, last_block) = archive_name.split_once("__")?;

    Some(ArchiveMetadata {
        file_name: format!("{archive_name}.tar.gz"),
        first_block: parse_archive_point(first_block)?,
        last_block: parse_archive_point(last_block)?,
    })
}

pub fn sorted_archives<'a>(archives: impl IntoIterator<Item = &'a String>) -> Vec<ArchiveMetadata> {
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
