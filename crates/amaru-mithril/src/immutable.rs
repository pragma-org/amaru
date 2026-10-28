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
    fs,
    io::{self, Read, Seek, SeekFrom},
    mem::size_of,
    num::NonZeroU64,
    path::{Path, PathBuf},
};

use amaru_kernel::{
    EraName, Hasher, HeaderHash, NetworkName, NetworkPoint, Point, Slot, cbor, extract_block_header_cbor, size,
};
use anyhow::{Context, Result as ImmutableResult};

const BYRON_EBB_TAG: u8 = 0;
const BYRON_BLOCK_TAG: u8 = EraName::Byron as u8;
const SHELLEY_BLOCK_TAG: u8 = EraName::Shelley as u8;
const DIJKSTRA_BLOCK_TAG: u8 = EraName::Dijkstra as u8;
const CBOR_PAIR_PREFIX: u8 = 0x82;
const IMMUTABLE_EXTENSIONS: [&str; 3] = ["chunk", "primary", "secondary"];

/// Number of regular slots per chunk in a uniform cardano-node immutable store.
///
/// Chunks are numbered from slot zero. If Byron EBBs are present, this must also be the
/// Byron epoch length: each EBB belongs to the chunk numbered by its epoch. The chunk
/// size remains fixed across hard forks, even when the ledger's epoch length changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct ImmutableChunkSize(NonZeroU64);

impl ImmutableChunkSize {
    /// Use the chunk size of the source database, in regular slots (excluding the EBB entry).
    pub fn new(slots_per_chunk: u64) -> io::Result<Self> {
        NonZeroU64::new(slots_per_chunk)
            .map(Self)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "immutable chunk size must be nonzero"))
    }

    /// The standard cardano-node layout for a known network.
    ///
    /// Uses ten times the security parameter from the network's built-in global parameters.
    /// For these networks, this matches the Byron genesis chunk layout. Custom networks must
    /// supply their source database's layout via [`Self::new`].
    pub fn for_network(network: NetworkName) -> io::Result<Self> {
        let parameters = network.as_global_parameters().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, format!("immutable chunk size not known for network {network}"))
        })?;
        let slots_per_chunk = parameters
            .consensus_security_param
            .checked_mul(10)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "immutable chunk size overflow"))?;
        Self::new(slots_per_chunk)
    }

    /// The chunk containing an absolute slot, in any era.
    pub fn chunk_for_slot(self, slot: Slot) -> u64 {
        slot.as_u64() / self.0
    }

    fn first_slot(self, chunk: u64) -> io::Result<Slot> {
        chunk
            .checked_mul(self.0.get())
            .map(Slot::from)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "chunk slot overflow"))
    }
}

/// A block read from the cardano-node immutable store.
#[derive(Debug)]
pub struct ImmutableBlock {
    pub hash: HeaderHash,
    pub header_cbor: Vec<u8>,
    pub raw_block: Vec<u8>,
}

#[derive(Clone, Copy)]
struct SecondaryEntry {
    offset: u64,
    hash: HeaderHash,
    /// Absolute slot for regular blocks, epoch number for Byron EBBs.
    slot_or_epoch: u64,
}

struct IndexedBlock {
    chunk: u64,
    index: SecondaryEntry,
    bytes: Vec<u8>,
}

impl IndexedBlock {
    fn verified_point(
        &self,
        chunk_size: ImmutableChunkSize,
        previous: Option<HeaderHash>,
    ) -> ImmutableResult<NetworkPoint> {
        let era = verify_historical_header(&self.bytes, self.index.hash, previous)?;
        let slot = if era == BYRON_EBB_TAG {
            anyhow::ensure!(
                self.index.slot_or_epoch == self.chunk,
                "EBB epoch {} does not match chunk {}",
                self.index.slot_or_epoch,
                self.chunk
            );
            chunk_size.first_slot(self.chunk)?
        } else {
            let slot = Slot::from(self.index.slot_or_epoch);
            anyhow::ensure!(
                chunk_size.chunk_for_slot(slot) == self.chunk,
                "indexed slot {slot} does not belong to chunk {} with layout {chunk_size:?}",
                self.chunk
            );
            slot
        };
        Ok(NetworkPoint::Specific(slot, self.index.hash))
    }
}

fn read_chunk(
    immutable_dir: &Path,
    chunk: u64,
) -> ImmutableResult<impl DoubleEndedIterator<Item = ImmutableResult<IndexedBlock>> + use<>> {
    let mut file = fs::File::open(immutable_file_path(immutable_dir, chunk, "chunk"))?;
    let chunk_len = file.metadata()?.len();
    let entries = read_secondary_entries(&immutable_file_path(immutable_dir, chunk, "secondary"), chunk_len)?;
    Ok((0..entries.len()).map(move |position| {
        let index = entries[position];
        let end = entries.get(position + 1).map_or(chunk_len, |next| next.offset);
        let mut bytes = vec![0; usize::try_from(end - index.offset)?];
        file.seek(SeekFrom::Start(index.offset))?;
        file.read_exact(&mut bytes)?;
        Ok(IndexedBlock { chunk, index, bytes })
    }))
}

/// Read opaque blocks and their indexed network points in immutable chain order.
///
/// The directory must come from a verified Mithril download starting at genesis. Each subsequent
/// header's parent must match the preceding hash, and every header must hash to its indexed hash.
/// Block bodies remain opaque. `chunk_size` must match the source database's uniform layout;
/// index entries inconsistent with that layout are rejected.
pub fn iter_immutable_raw_blocks(
    immutable_dir: &Path,
    chunk_size: ImmutableChunkSize,
) -> io::Result<impl Iterator<Item = ImmutableResult<(NetworkPoint, Vec<u8>)>> + use<>> {
    Ok(immutable_raw_blocks_iter(immutable_dir, list_immutable_chunks(immutable_dir)?).scan(
        None,
        move |previous, block| {
            Some(block.and_then(|block| {
                let point = block.verified_point(chunk_size, *previous).with_context(|| {
                    format!("historical block in chunk {} at offset {}", block.chunk, block.index.offset)
                })?;
                *previous = Some(point.hash());
                Ok((point, block.bytes))
            }))
        },
    ))
}

/// Verify the header hash and parent link without interpreting the block body.
fn verify_historical_header(bytes: &[u8], hash: HeaderHash, previous: Option<HeaderHash>) -> ImmutableResult<u8> {
    let mut decoder = cbor::Decoder::new(bytes);
    decoder.array()?;
    let era = decoder.u8()?;
    decoder.array()?;
    let start = decoder.position();
    decoder.skip()?;
    let header = &bytes[start..decoder.position()];
    let mut decoder = cbor::Decoder::new(header);
    decoder.array()?;
    match era {
        BYRON_EBB_TAG | BYRON_BLOCK_TAG => {
            // Byron headers start with protocol magic, then the parent hash.
            decoder.u32()?;
        }
        SHELLEY_BLOCK_TAG..=DIJKSTRA_BLOCK_TAG => {
            // Shelley onwards: header body starts with block number, slot, parent hash.
            decoder.array()?;
            decoder.u64()?;
            decoder.u64()?;
        }
        _ => anyhow::bail!("unsupported historical block era"),
    }
    let parent = decoder.decode::<Option<HeaderHash>>()?;
    let mut hasher = Hasher::<256>::new();
    if matches!(era, BYRON_EBB_TAG | BYRON_BLOCK_TAG) {
        // Byron hashes the CBOR pair [block type, header], including for epoch boundary blocks.
        hasher.input(&[CBOR_PAIR_PREFIX, era]);
    }
    hasher.input(header);
    let actual_hash = hasher.finalize();
    anyhow::ensure!(actual_hash == hash, "historical header hash mismatch: indexed {hash}, actual {actual_hash}");
    if let Some(previous) = previous {
        anyhow::ensure!(parent == Some(previous), "broken historical chain: expected parent {previous}");
    }
    Ok(era)
}

/// Iterates over every decodable block in immutable chunk order.
///
/// Returns an error when the immutable directory cannot be read. Errors encountered while reading individual chunks
/// are yielded by the returned iterator.
pub fn iter_immutable_blocks(
    immutable_dir: &Path,
) -> io::Result<impl Iterator<Item = ImmutableResult<ImmutableBlock>> + use<>> {
    Ok(immutable_raw_blocks_iter(immutable_dir, list_immutable_chunks(immutable_dir)?).map(|block| {
        let block = block?;
        let header_cbor = extract_block_header_cbor(&block.bytes)
            .with_context(|| {
                format!("failed to decode immutable block in chunk {} at offset {}", block.chunk, block.index.offset)
            })?
            .to_vec();
        Ok(ImmutableBlock { hash: Hasher::<256>::hash(&header_cbor), header_cbor, raw_block: block.bytes })
    }))
}

/// Iterates over raw blocks strictly after `point` on `network`.
///
/// This includes the newest numbered chunk and is intended for a verified Mithril database, whose
/// downloaded immutable files are final. `Point::Origin` starts at the first block.
pub fn read_blocks_after_point(
    immutable_dir: &Path,
    network: NetworkName,
    point: Point,
) -> ImmutableResult<impl Iterator<Item = ImmutableResult<Vec<u8>>>> {
    let chunks = list_immutable_chunks(immutable_dir)?;
    let first_chunk = match point {
        Point::Origin => None,
        Point::Specific(slot, _, _) => Some(ImmutableChunkSize::for_network(network)?.chunk_for_slot(slot)),
    };
    let chunks =
        chunks.into_iter().skip_while(|chunk| first_chunk.is_some_and(|first_chunk| *chunk < first_chunk)).collect();
    let mut blocks = immutable_raw_blocks_iter(immutable_dir, chunks).map(|block| block.map(|block| block.bytes));
    if let Point::Specific(..) = point {
        consume_through_point(&mut blocks, point)?;
    }
    Ok(blocks)
}

/// Find the last block in a verified immutable database, optionally stopping at a slot.
///
/// Without a slot limit, only the final block is decoded. With a limit, chunks are scanned
/// backwards until the last eligible block is found.
pub fn last_immutable_point(immutable_dir: &Path, until_slot: Option<Slot>) -> ImmutableResult<Option<Point>> {
    list_immutable_chunks(immutable_dir)?
        .into_iter()
        .rev()
        .map(|chunk| last_point_in_chunk(immutable_dir, chunk, until_slot))
        .find_map(Result::transpose)
        .transpose()
}

fn last_point_in_chunk(immutable_dir: &Path, chunk: u64, until_slot: Option<Slot>) -> ImmutableResult<Option<Point>> {
    read_chunk(immutable_dir, chunk)?
        .rev()
        .map(|block| {
            let point = parse_block_point(&block?.bytes)?;
            Ok(until_slot.is_none_or(|limit| point.slot_or_default() <= limit).then_some(point))
        })
        .find_map(Result::transpose)
        .transpose()
}

fn immutable_raw_blocks_iter(
    immutable_dir: &Path,
    chunks: Vec<u64>,
) -> impl Iterator<Item = ImmutableResult<IndexedBlock>> + use<> {
    let immutable_dir = immutable_dir.to_path_buf();
    let mut chunks = chunks.into_iter();
    let mut current = None;
    std::iter::from_fn(move || {
        loop {
            if current.is_none() {
                current = match read_chunk(&immutable_dir, chunks.next()?) {
                    Ok(blocks) => Some(blocks),
                    Err(error) => return Some(Err(error)),
                };
            }
            if let Some(block) = current.as_mut().and_then(Iterator::next) {
                return Some(block);
            }
            current = None;
        }
    })
}

fn list_immutable_chunks(immutable_dir: &Path) -> Result<Vec<u64>, io::Error> {
    fs::read_dir(immutable_dir)?
        .map(|entry| {
            entry.map(|entry| {
                immutable_file_parts(&entry.path())
                    .and_then(|(chunk, extension)| (extension == "chunk").then_some(chunk))
            })
        })
        .filter_map(Result::transpose)
        .collect::<Result<BTreeSet<_>, _>>()
        .map(|chunks| chunks.into_iter().collect())
}

fn immutable_file_parts(path: &Path) -> Option<(u64, &'static str)> {
    let extension = path.extension()?.to_str()?;
    let extension = IMMUTABLE_EXTENSIONS.into_iter().find(|expected| *expected == extension)?;
    Some((path.file_stem()?.to_str()?.parse().ok()?, extension))
}

fn consume_through_point(
    blocks: &mut impl Iterator<Item = ImmutableResult<Vec<u8>>>,
    point: Point,
) -> ImmutableResult<()> {
    for block in blocks {
        let block_point = parse_block_point(&block?)?;
        if block_point == point {
            return Ok(());
        }
        if block_point.slot_or_default() > point.slot_or_default() {
            break;
        }
    }
    Err(anyhow::anyhow!("cannot find block in immutable storage: {point}"))
}

/// Validates the immutable cache and returns a safe chunk from which a download can resume.
///
/// Numbered immutable files must form a continuous range, every chunk must have its `.chunk`,
/// `.primary`, and `.secondary` files, and every secondary index must describe increasing block
/// offsets within its chunk. The returned boundary is one chunk before the cached tail and never
/// later than one chunk before `resume_point`.
pub(crate) fn validated_download_boundary(
    immutable_dir: &Path,
    network: NetworkName,
    resume_point: Point,
) -> ImmutableResult<u64> {
    let cache_boundary = validate_immutable_files(immutable_dir)?.map(|last_chunk| last_chunk.saturating_sub(1));
    let resume_boundary =
        ImmutableChunkSize::for_network(network)?.chunk_for_slot(resume_point.slot_or_default()).saturating_sub(1);
    Ok(cache_boundary.map_or(resume_boundary, |boundary| boundary.min(resume_boundary)))
}

/// Validates the immutable cache and checks that `resume_point` is present exactly.
pub(crate) fn validate_immutable_resume_point(
    immutable_dir: &Path,
    network: NetworkName,
    resume_point: Point,
) -> ImmutableResult<()> {
    validate_immutable_files(immutable_dir)?
        .ok_or_else(|| anyhow::anyhow!("immutable cache is empty: {}", immutable_dir.display()))?;
    drop(read_blocks_after_point(immutable_dir, network, resume_point)?);
    Ok(())
}

pub(crate) fn validate_immutable_files(immutable_dir: &Path) -> ImmutableResult<Option<u64>> {
    let entries = match fs::read_dir(immutable_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut files_by_chunk = BTreeMap::<u64, BTreeSet<&'static str>>::new();

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let Some((chunk, extension)) = immutable_file_parts(&path) else {
            continue;
        };
        if !entry.file_type()?.is_file() {
            return Err(anyhow::anyhow!("expected a regular file at {}", path.display()));
        }
        files_by_chunk.entry(chunk).or_default().insert(extension);
    }

    for (previous, next) in files_by_chunk.keys().zip(files_by_chunk.keys().skip(1)) {
        if previous.checked_add(1) != Some(*next) {
            return Err(anyhow::anyhow!("immutable chunk sequence is not continuous between {previous} and {next}"));
        }
    }

    for (chunk, extensions) in &files_by_chunk {
        for extension in IMMUTABLE_EXTENSIONS {
            if !extensions.contains(extension) {
                return Err(anyhow::anyhow!(
                    "missing immutable file {}",
                    immutable_file_path(immutable_dir, *chunk, extension).display()
                ));
            }
        }
        let chunk_path = immutable_file_path(immutable_dir, *chunk, "chunk");
        let chunk_len = fs::metadata(chunk_path)?.len();
        read_secondary_entries(&immutable_file_path(immutable_dir, *chunk, "secondary"), chunk_len)?;
    }

    Ok(files_by_chunk.last_key_value().map(|(chunk, _)| *chunk))
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

pub(crate) fn immutable_chunk_is_complete(immutable_dir: &Path, chunk: u64) -> io::Result<bool> {
    IMMUTABLE_EXTENSIONS.into_iter().try_fold(true, |complete, extension| {
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

const SECONDARY_HASH_OFFSET: usize = 16;
const SECONDARY_SLOT_OFFSET: usize = SECONDARY_HASH_OFFSET + size::HEADER;
const SECONDARY_ENTRY_SIZE: usize = SECONDARY_SLOT_OFFSET + size_of::<u64>();

fn read_secondary_entries(secondary_path: &Path, chunk_len: u64) -> io::Result<Vec<SecondaryEntry>> {
    let secondary = fs::read(secondary_path)?;
    parse_secondary_entries(&secondary, chunk_len)
        .map_err(|error| io::Error::new(error.kind(), format!("{error} in {}", secondary_path.display())))
}

fn parse_secondary_entries(secondary: &[u8], chunk_len: u64) -> io::Result<Vec<SecondaryEntry>> {
    if !secondary.len().is_multiple_of(SECONDARY_ENTRY_SIZE) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid immutable secondary index size: {} bytes", secondary.len()),
        ));
    }

    let entries = secondary
        .as_chunks::<SECONDARY_ENTRY_SIZE>()
        .0
        .iter()
        .map(|entry| SecondaryEntry {
            offset: u64::from_be_bytes(std::array::from_fn(|i| entry[i])),
            hash: HeaderHash::from(std::array::from_fn(|i| entry[SECONDARY_HASH_OFFSET + i])),
            slot_or_epoch: u64::from_be_bytes(std::array::from_fn(|i| entry[SECONDARY_SLOT_OFFSET + i])),
        })
        .collect::<Vec<_>>();
    let valid = entries.first().map_or(chunk_len == 0, |first| first.offset == 0)
        && entries.last().is_none_or(|last| last.offset < chunk_len)
        && entries.windows(2).all(|pair| pair[0].offset < pair[1].offset);
    if !valid {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid immutable secondary index offsets"));
    }

    Ok(entries)
}

fn parse_block_point(input: &[u8]) -> Result<Point, cbor::decode::Error> {
    let header = extract_block_header_cbor(input)?;
    let mut decoder = cbor::Decoder::new(header);
    decoder.array()?;
    decoder.array()?;
    let height = decoder.u64()?;
    let slot = decoder.u64()?;
    Ok(Point::Specific(slot.into(), Hasher::<256>::hash(header), height.into()))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{fs, mem::size_of, path::Path};

    use amaru_kernel::{Hasher, NetworkName, Point, cbor};
    use tempfile::TempDir;

    use super::{
        SECONDARY_ENTRY_SIZE, first_missing_immutable_chunk, immutable_file_path, iter_immutable_blocks,
        last_immutable_point, read_blocks_after_point, validate_immutable_resume_point, validated_download_boundary,
    };

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
        (Point::Specific(slot.into(), hash, slot.into()), bytes)
    }

    fn write_chunk(immutable_dir: &Path, chunk_number: u64, blocks: &[Vec<u8>]) {
        let mut chunk = Vec::new();
        let mut secondary = Vec::new();
        for block in blocks {
            let mut entry = [0_u8; SECONDARY_ENTRY_SIZE];
            entry[..size_of::<u64>()].copy_from_slice(&(chunk.len() as u64).to_be_bytes());
            secondary.extend_from_slice(&entry);
            chunk.extend_from_slice(block);
        }
        fs::write(immutable_file_path(immutable_dir, chunk_number, "chunk"), chunk).unwrap();
        fs::write(immutable_file_path(immutable_dir, chunk_number, "primary"), []).unwrap();
        fs::write(immutable_file_path(immutable_dir, chunk_number, "secondary"), secondary).unwrap();
    }

    pub(crate) fn immutable_store() -> (TempDir, Vec<(Point, Vec<u8>)>) {
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

        assert_eq!(first_missing_immutable_chunk(&immutable_dir).unwrap(), 0);

        fs::create_dir(&immutable_dir).unwrap();
        write_chunk(&immutable_dir, 0, &[]);
        write_chunk(&immutable_dir, 1, &[]);
        fs::write(immutable_file_path(&immutable_dir, 2, "chunk"), []).unwrap();

        assert_eq!(first_missing_immutable_chunk(&immutable_dir).unwrap(), 2);
    }

    #[test]
    fn reads_blocks_after_a_point_across_empty_chunks() {
        let (dir, blocks) = immutable_store();

        for (point, start) in [(Point::Origin, 0), (blocks[1].0, 2), (blocks[3].0, 4)] {
            let actual = read_blocks_after_point(dir.path(), NetworkName::Preprod, point)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(actual, blocks[start..].iter().map(|(_, block)| block.clone()).collect::<Vec<_>>());
        }
    }

    #[test]
    fn finds_last_point_with_empty_chunks_and_a_slot_limit() {
        let (dir, blocks) = immutable_store();
        write_chunk(dir.path(), 4, &[]);

        assert_eq!(last_immutable_point(dir.path(), None).unwrap(), Some(blocks[4].0));
        assert_eq!(last_immutable_point(dir.path(), Some(43_201.into())).unwrap(), Some(blocks[2].0));
        assert_eq!(last_immutable_point(dir.path(), Some(0.into())).unwrap(), None);
    }

    #[test]
    fn download_boundary_does_not_advance_past_resume_point() {
        let (dir, blocks) = immutable_store();

        assert_eq!(validated_download_boundary(dir.path(), NetworkName::Preprod, blocks[1].0).unwrap(), 0);
        assert_eq!(validated_download_boundary(dir.path(), NetworkName::Preprod, blocks[4].0).unwrap(), 2);
    }

    #[test]
    fn rejects_non_continuous_chunks() {
        let (dir, blocks) = immutable_store();
        for extension in ["chunk", "primary", "secondary"] {
            fs::remove_file(immutable_file_path(dir.path(), 1, extension)).unwrap();
        }

        assert!(validated_download_boundary(dir.path(), NetworkName::Preprod, blocks[1].0).is_err());
    }

    #[test]
    fn rejects_missing_immutable_files() {
        let (dir, blocks) = immutable_store();
        fs::remove_file(immutable_file_path(dir.path(), 1, "primary")).unwrap();

        assert!(validated_download_boundary(dir.path(), NetworkName::Preprod, blocks[1].0).is_err());
    }

    #[test]
    fn rejects_invalid_secondary_offsets() {
        let (dir, blocks) = immutable_store();
        let secondary_path = immutable_file_path(dir.path(), 0, "secondary");
        let mut secondary = fs::read(&secondary_path).unwrap();
        secondary[56..64].copy_from_slice(&0_u64.to_be_bytes());
        fs::write(secondary_path, secondary).unwrap();

        assert!(validated_download_boundary(dir.path(), NetworkName::Preprod, blocks[1].0).is_err());
    }

    #[test]
    fn validates_the_exact_resume_point() {
        let (dir, blocks) = immutable_store();
        validate_immutable_resume_point(dir.path(), NetworkName::Preprod, blocks[1].0).unwrap();

        let wrong_point = Point::Specific(blocks[1].0.slot_or_default(), [0; 32].into(), blocks[1].0.block_height());
        assert!(validate_immutable_resume_point(dir.path(), NetworkName::Preprod, wrong_point).is_err());
    }

    #[test]
    fn reports_malformed_blocks_with_their_location() {
        let dir = TempDir::new().unwrap();
        write_chunk(dir.path(), 7, &[vec![0xff]]);

        let error = iter_immutable_blocks(dir.path()).unwrap().next().unwrap().unwrap_err();

        assert_eq!(error.to_string(), "failed to decode immutable block in chunk 7 at offset 0");
    }
}
