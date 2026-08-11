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
    collections::BTreeSet,
    error::Error,
    fs,
    io::{self, Read, Seek, SeekFrom},
    mem::size_of,
    path::{Path, PathBuf},
    vec::IntoIter,
};

use amaru_kernel::{GlobalParameters, Hasher, HeaderHash, NetworkName, Point, extract_block_header_cbor};

use crate::parse_header_slot_and_hash;

type ImmutableResult<T> = Result<T, Box<dyn Error>>;

/// A block read from the cardano-node immutable store.
#[derive(Debug)]
pub struct ImmutableBlock {
    pub hash: HeaderHash,
    pub header_cbor: Vec<u8>,
    pub raw_block: Vec<u8>,
}

/// Iterator over blocks in sorted immutable chunk order, reading one chunk at a time.
pub struct ImmutableBlocksIter {
    inner: ImmutableRawBlocksIter,
}

struct ImmutableRawBlocksIter {
    chunks: IntoIter<u64>,
    immutable_dir: PathBuf,
    current_chunk: Option<ChunkBlockIter>,
}

struct ChunkBlockIter {
    chunk_file: fs::File,
    secondary_path: PathBuf,
    offsets: std::iter::Peekable<IntoIter<u64>>,
    chunk_len: u64,
}

impl Iterator for ImmutableBlocksIter {
    type Item = ImmutableResult<ImmutableBlock>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.by_ref().find_map(|block| match block {
            Ok(raw_block) => decode_immutable_block(raw_block).map(Ok),
            Err(error) => Some(Err(error)),
        })
    }
}

impl ChunkBlockIter {
    fn open(immutable_dir: &Path, chunk: u64) -> ImmutableResult<Self> {
        let chunk_path = immutable_file_path(immutable_dir, chunk, "chunk");
        let secondary_path = immutable_file_path(immutable_dir, chunk, "secondary");
        let offsets = read_secondary_offsets(&secondary_path)?.into_iter().peekable();
        let chunk_file = fs::File::open(chunk_path)?;
        let chunk_len = chunk_file.metadata()?.len();
        Ok(Self { chunk_file, secondary_path, offsets, chunk_len })
    }

    fn read_block(&mut self, start: u64, end: u64) -> ImmutableResult<Vec<u8>> {
        let block_len = end.checked_sub(start).ok_or_else(|| {
            format!("invalid immutable offsets in {}: start={start}, end={end}", self.secondary_path.display())
        })?;
        let mut raw_block = vec![0; usize::try_from(block_len)?];
        self.chunk_file.seek(SeekFrom::Start(start))?;
        self.chunk_file.read_exact(&mut raw_block)?;
        Ok(raw_block)
    }
}

impl Iterator for ChunkBlockIter {
    type Item = ImmutableResult<Vec<u8>>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let start = self.offsets.next()?;
            let end = self.offsets.peek().copied().unwrap_or(self.chunk_len);
            if end == start {
                continue;
            }
            return Some(self.read_block(start, end));
        }
    }
}

impl Iterator for ImmutableRawBlocksIter {
    type Item = ImmutableResult<Vec<u8>>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(chunk) = &mut self.current_chunk
                && let Some(block) = chunk.next()
            {
                return Some(block);
            }
            let chunk = self.chunks.next()?;
            match ChunkBlockIter::open(&self.immutable_dir, chunk) {
                Ok(chunk) => self.current_chunk = Some(chunk),
                Err(error) => return Some(Err(error)),
            }
        }
    }
}

/// Iterates over every decodable block in immutable chunk order.
///
/// Returns an error when the immutable directory cannot be read. Errors encountered while reading individual chunks
/// are yielded by the returned iterator.
pub fn iter_immutable_blocks(immutable_dir: &Path) -> Result<ImmutableBlocksIter, io::Error> {
    Ok(ImmutableBlocksIter { inner: immutable_raw_blocks_iter(immutable_dir, list_immutable_chunks(immutable_dir)?) })
}

/// Iterates over raw blocks strictly after `point` on `network`, excluding the newest immutable chunk.
///
/// The newest chunk is omitted because it may still change. `Point::Origin` starts at the first block. Returns an
/// error when the directory or a chunk cannot be read, a block cannot be decoded while locating `point`, or `point`
/// is absent from the stable chunks.
pub fn read_stable_blocks_after_point(
    immutable_dir: &Path,
    network: NetworkName,
    point: Point,
) -> ImmutableResult<impl Iterator<Item = ImmutableResult<Vec<u8>>>> {
    let chunks = stable_immutable_chunks_from_point(immutable_dir, network, point)?;
    let mut blocks = immutable_raw_blocks_iter(immutable_dir, chunks);
    if let Point::Specific(slot, _) = point {
        consume_through_point(&mut blocks, point, slot.as_u64())?;
    }
    Ok(blocks)
}

fn immutable_raw_blocks_iter(immutable_dir: &Path, chunks: Vec<u64>) -> ImmutableRawBlocksIter {
    ImmutableRawBlocksIter {
        chunks: chunks.into_iter(),
        immutable_dir: immutable_dir.to_path_buf(),
        current_chunk: None,
    }
}

fn list_immutable_chunks(immutable_dir: &Path) -> Result<Vec<u64>, io::Error> {
    fs::read_dir(immutable_dir)?
        .map(|entry| entry.map(|entry| immutable_chunk_number(&entry.path())))
        .filter_map(Result::transpose)
        .collect::<Result<BTreeSet<_>, _>>()
        .map(|chunks| chunks.into_iter().collect())
}

fn immutable_chunk_number(path: &Path) -> Option<u64> {
    (path.extension().and_then(|extension| extension.to_str()) == Some("chunk"))
        .then_some(path)
        .and_then(|path| path.file_stem()?.to_str()?.parse().ok())
}

fn stable_immutable_chunks_from_point(
    immutable_dir: &Path,
    network: NetworkName,
    point: Point,
) -> ImmutableResult<Vec<u64>> {
    let chunks = list_immutable_chunks(immutable_dir)?;
    let stable_chunk_count = chunks.len().saturating_sub(1);
    let first_chunk = match point {
        Point::Origin => None,
        Point::Specific(slot, _) => Some(chunk_for_slot(network, slot.as_u64())?),
    };
    Ok(chunks
        .into_iter()
        .take(stable_chunk_count)
        .skip_while(|chunk| first_chunk.is_some_and(|first_chunk| *chunk < first_chunk))
        .collect())
}

fn consume_through_point(blocks: &mut ImmutableRawBlocksIter, point: Point, target_slot: u64) -> ImmutableResult<()> {
    let reached = blocks
        .by_ref()
        .map(|block| {
            let block = block?;
            let parsed = parse_header_slot_and_hash(&block)?;
            let block_point = Point::Specific(parsed.slot.into(), parsed.header_hash.into());
            Ok((block_point, parsed.slot))
        })
        .find(|candidate: &ImmutableResult<(Point, u64)>| match candidate {
            Ok((block_point, slot)) => *block_point == point || *slot > target_slot,
            Err(_) => true,
        })
        .transpose()?;

    match reached {
        Some((block_point, _)) if block_point == point => Ok(()),
        _ => Err(point_not_found(point)),
    }
}

fn point_not_found(point: Point) -> Box<dyn Error> {
    format!("cannot find block in immutable storage: {point}").into()
}

/// Returns the chunk immediately before the greatest immutable chunk number.
///
/// Returns `None` when the directory does not exist or contains no numbered `.chunk` files. Starting one chunk back
/// ensures that a partially downloaded or recently changed tail is fetched again.
pub fn get_latest_chunk(immutable_dir: &Path) -> Result<Option<u64>, io::Error> {
    match list_immutable_chunks(immutable_dir) {
        Ok(chunks) => Ok(chunks.into_iter().max().map(|chunk| chunk.saturating_sub(1))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Returns the first chunk number without a complete `.chunk`, `.primary`, and `.secondary` file set.
///
/// A missing directory is treated as an empty immutable store. Existing non-file entries and filesystem errors are
/// returned to the caller.
pub fn first_missing_immutable_chunk(immutable_dir: &Path) -> Result<u64, io::Error> {
    let mut chunk = 0_u64;
    while immutable_chunk_is_complete(immutable_dir, chunk)? {
        chunk = chunk.checked_add(1).ok_or_else(|| io::Error::other("immutable chunk index overflows u64"))?;
    }
    Ok(chunk)
}

fn immutable_chunk_is_complete(immutable_dir: &Path, chunk: u64) -> io::Result<bool> {
    ["chunk", "primary", "secondary"].into_iter().try_fold(true, |complete, extension| {
        if !complete {
            return Ok(false);
        }
        let path = immutable_file_path(immutable_dir, chunk, extension);
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => Ok(true),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected a regular file at {}", path.display()),
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    })
}

fn immutable_file_path(immutable_dir: &Path, chunk: u64, extension: &str) -> PathBuf {
    immutable_dir.join(format!("{chunk:05}.{extension}"))
}

/// Returns the immutable chunk containing `slot` for `network`.
///
/// Returns an error when global parameters are unavailable for the requested network.
pub fn chunk_for_slot(network: NetworkName, slot: u64) -> anyhow::Result<u64> {
    let global_parameters: &GlobalParameters = network
        .as_global_parameters()
        .ok_or_else(|| anyhow::anyhow!("GlobalParameters not known for network name `{}`", network))?;
    let slots_per_chunk = 10 * global_parameters.consensus_security_param;
    Ok(slot / slots_per_chunk)
}

/// Selects the first immutable chunk to download when resuming block packaging.
///
/// A locally known chunk takes precedence. Otherwise, downloading starts one chunk before the chunk containing the
/// resume point so that the resume block is available for exact point matching.
pub fn from_chunk_for_resume_point(
    network: NetworkName,
    latest_chunk: Option<u64>,
    resume_point: Point,
) -> anyhow::Result<u64> {
    match latest_chunk {
        Some(latest_chunk) => Ok(latest_chunk),
        None => Ok(chunk_for_slot(network, resume_point.slot_or_default().into())?.saturating_sub(1)),
    }
}

const SECONDARY_ENTRY_SIZE: usize = 56;

fn read_secondary_offsets(secondary_path: &Path) -> io::Result<Vec<u64>> {
    let secondary = fs::read(secondary_path)?;
    if secondary.len() % SECONDARY_ENTRY_SIZE != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "invalid immutable secondary index size for {}: {} bytes",
                secondary_path.display(),
                secondary.len()
            ),
        ));
    }

    Ok(secondary.as_chunks::<SECONDARY_ENTRY_SIZE>().0.iter().map(secondary_block_offset).collect())
}

fn secondary_block_offset(entry: &[u8; SECONDARY_ENTRY_SIZE]) -> u64 {
    let mut offset = [0; size_of::<u64>()];
    offset.copy_from_slice(&entry[..size_of::<u64>()]);
    u64::from_be_bytes(offset)
}

fn decode_immutable_block(raw_block: Vec<u8>) -> Option<ImmutableBlock> {
    let header_cbor = extract_block_header_cbor(&raw_block).ok()?.to_vec();
    let hash = Hasher::<256>::hash(&header_cbor);
    Some(ImmutableBlock { hash, header_cbor, raw_block })
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use amaru_kernel::{Hasher, NetworkName, Point, cbor};
    use tempfile::TempDir;

    use super::{first_missing_immutable_chunk, get_latest_chunk, immutable_file_path, read_stable_blocks_after_point};

    fn block(slot: u64) -> (Point, Vec<u8>) {
        let mut encoder = cbor::Encoder::new(Vec::new());
        encoder.array(2).unwrap();
        encoder.u8(1).unwrap();
        encoder.array(1).unwrap();
        let header_start = encoder.writer().len();
        encoder.array(1).unwrap();
        encoder.array(2).unwrap();
        encoder.u64(slot).unwrap();
        encoder.u64(slot).unwrap();
        let bytes = encoder.into_writer();
        let hash = Hasher::<256>::hash(&bytes[header_start..]);
        (Point::Specific(slot.into(), hash), bytes)
    }

    fn write_chunk(immutable_dir: &Path, chunk_number: u64, blocks: &[Vec<u8>]) {
        let mut chunk = Vec::new();
        let mut secondary = Vec::new();
        for block in blocks {
            let mut entry = [0_u8; 56];
            entry[0..8].copy_from_slice(&(chunk.len() as u64).to_be_bytes());
            secondary.extend_from_slice(&entry);
            chunk.extend_from_slice(block);
        }
        fs::write(immutable_file_path(immutable_dir, chunk_number, "chunk"), chunk).unwrap();
        fs::write(immutable_file_path(immutable_dir, chunk_number, "primary"), []).unwrap();
        fs::write(immutable_file_path(immutable_dir, chunk_number, "secondary"), secondary).unwrap();
    }

    fn immutable_store() -> (TempDir, Vec<(Point, Vec<u8>)>) {
        let dir = TempDir::new().unwrap();
        let blocks = [1, 2, 43_201, 43_202, 64_801].into_iter().map(block).collect::<Vec<_>>();
        write_chunk(dir.path(), 0, &[blocks[0].1.clone(), blocks[1].1.clone()]);
        write_chunk(dir.path(), 1, &[]);
        write_chunk(dir.path(), 2, &[blocks[2].1.clone(), blocks[3].1.clone()]);
        write_chunk(dir.path(), 3, &[blocks[4].1.clone()]);
        (dir, blocks)
    }

    #[test]
    fn finds_safe_download_boundaries() {
        let dir = TempDir::new().unwrap();
        let immutable_dir = dir.path().join("immutable");

        assert_eq!(get_latest_chunk(&immutable_dir).unwrap(), None);
        assert_eq!(first_missing_immutable_chunk(&immutable_dir).unwrap(), 0);

        fs::create_dir(&immutable_dir).unwrap();
        write_chunk(&immutable_dir, 0, &[]);
        write_chunk(&immutable_dir, 1, &[]);
        fs::write(immutable_file_path(&immutable_dir, 2, "chunk"), []).unwrap();

        assert_eq!(get_latest_chunk(&immutable_dir).unwrap(), Some(1));
        assert_eq!(first_missing_immutable_chunk(&immutable_dir).unwrap(), 2);
    }

    #[test]
    fn reads_blocks_after_a_point_across_empty_chunks() {
        let (dir, blocks) = immutable_store();

        let actual = read_stable_blocks_after_point(dir.path(), NetworkName::Preprod, blocks[1].0)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(actual, blocks[2..4].iter().map(|(_, block)| block.clone()).collect::<Vec<_>>());
    }

    #[test]
    fn stable_reader_omits_the_last_chunk() {
        let (dir, blocks) = immutable_store();

        let actual = read_stable_blocks_after_point(dir.path(), NetworkName::Preprod, blocks[3].0)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert!(actual.is_empty());
    }

    #[test]
    fn origin_includes_the_first_block() {
        let (dir, blocks) = immutable_store();

        let actual = read_stable_blocks_after_point(dir.path(), NetworkName::Preprod, Point::Origin)
            .unwrap()
            .next()
            .unwrap()
            .unwrap();

        assert_eq!(actual, blocks[0].1);
    }

    #[test]
    fn rejects_a_point_with_an_unknown_hash() {
        let (dir, blocks) = immutable_store();
        let point = Point::Specific(blocks[1].0.slot_or_default(), [0; 32].into());

        assert!(read_stable_blocks_after_point(dir.path(), NetworkName::Preprod, point).is_err());
    }
}
