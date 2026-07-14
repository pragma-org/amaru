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

pub use amaru_kernel::extract_block_header_cbor;
use amaru_kernel::{Hasher, cbor, extract_block_header_cbor as _extract_block_header_cbor};

mod archive;
mod download;
mod immutable;

pub use archive::{
    ArchiveMetadata, BLOCKS_PER_ARCHIVE, archive_name_for_blocks, latest_archive, list_existing_archives,
    package_blocks, parse_archive_metadata, resume_point_for_archives, sorted_archives,
};
pub use download::download_from_mithril;
pub use immutable::{
    ImmutableBlock, ImmutableBlocksIter, chunk_for_slot, first_missing_immutable_chunk, from_chunk_for_resume_point,
    get_latest_chunk, iter_immutable_blocks,
};

#[derive(Debug)]
pub struct ParsedHeader {
    pub slot: u64,
    pub header_hash: [u8; 32],
}

pub fn parse_header_slot_and_hash(input: &[u8]) -> Result<ParsedHeader, cbor::decode::Error> {
    let header_body_cbor = _extract_block_header_cbor(input)?;

    let header_hash = *Hasher::<256>::hash(header_body_cbor);
    let mut body = cbor::Decoder::new(header_body_cbor);

    body.array()?;
    body.array()?;
    body.u64()?;
    let slot = body.u64()?;
    Ok(ParsedHeader { slot, header_hash })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use amaru_kernel::Point;

    use crate::archive::{
        ArchiveMetadata, archive_name_for_blocks, latest_archive, parse_archive_metadata, sorted_archives,
    };

    fn parse_archive_bounds(archive_name: &str) -> Option<(Point, Point)> {
        let metadata = parse_archive_metadata(archive_name)?;
        Some((metadata.first_block, metadata.last_block))
    }

    fn latest_archive_end_point<'a>(archives: impl IntoIterator<Item = &'a String>) -> Option<Point> {
        sorted_archives(archives).into_iter().last().map(|a| a.last_block)
    }

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
