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
    fs,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use amaru_kernel::{GlobalParameters, Hasher, HeaderHash, NetworkName, extract_block_header_cbor};

/// A block read from the cardano-node immutable store.
#[derive(Debug)]
pub struct ImmutableBlock {
    pub hash: HeaderHash,
    pub header_cbor: Vec<u8>,
    pub raw_block: Vec<u8>,
}

/// Iterator over blocks in sorted immutable chunk order, reading one chunk at a time.
pub struct ImmutableBlocksIter {
    pub(crate) chunk_names: std::vec::IntoIter<String>,
    pub(crate) immutable_dir: PathBuf,
    pub(crate) current_chunk: std::vec::IntoIter<ImmutableBlock>,
}

impl Iterator for ImmutableBlocksIter {
    type Item = Result<ImmutableBlock, Box<dyn std::error::Error>>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(block) = self.current_chunk.next() {
                return Some(Ok(block));
            }
            let chunk_name = self.chunk_names.next()?;
            match read_chunk_blocks(&self.immutable_dir, &chunk_name) {
                Ok(blocks) => self.current_chunk = blocks.into_iter(),
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

/// Iterate over all blocks in the immutable directory in slot order.
pub fn iter_immutable_blocks(immutable_dir: &Path) -> Result<ImmutableBlocksIter, io::Error> {
    let mut chunk_names: Vec<String> = fs::read_dir(immutable_dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("chunk") {
                return None;
            }
            path.file_stem().and_then(|stem| stem.to_str()).map(str::to_owned)
        })
        .collect();
    chunk_names.sort_unstable();
    Ok(ImmutableBlocksIter {
        chunk_names: chunk_names.into_iter(),
        immutable_dir: immutable_dir.to_path_buf(),
        current_chunk: Vec::new().into_iter(),
    })
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
    let global_parameters: &GlobalParameters = network
        .as_global_parameters()
        .ok_or_else(|| anyhow::anyhow!("GlobalParameters not know for network name `{}`", network))?;
    let slots_per_chunk = 10 * global_parameters.consensus_security_param;
    Ok(slot / slots_per_chunk)
}

pub fn from_chunk_for_resume_point(
    network: NetworkName,
    latest_chunk: Option<u64>,
    resume_point: amaru_kernel::Point,
) -> anyhow::Result<u64> {
    if let Some(latest) = latest_chunk {
        return Ok(latest);
    }
    Ok(chunk_for_slot(network, resume_point.slot_or_default().into())?.saturating_sub(1))
}

fn read_secondary_offsets(secondary_path: &Path) -> Result<Vec<u64>, Box<dyn std::error::Error>> {
    const SECONDARY_ENTRY_SIZE: usize = 56;

    let secondary = fs::read(secondary_path)?;
    if secondary.len() % SECONDARY_ENTRY_SIZE != 0 {
        return Err(format!(
            "invalid immutable secondary index size for {}: {} bytes",
            secondary_path.display(),
            secondary.len()
        )
        .into());
    }

    let mut offsets = Vec::with_capacity(secondary.len() / SECONDARY_ENTRY_SIZE);
    for entry in secondary.chunks_exact(SECONDARY_ENTRY_SIZE) {
        let block_offset = u64::from_be_bytes(entry[0..8].try_into()?);
        offsets.push(block_offset);
    }

    Ok(offsets)
}

fn read_chunk_blocks(
    immutable_dir: &Path,
    chunk_name: &str,
) -> Result<Vec<ImmutableBlock>, Box<dyn std::error::Error>> {
    let chunk_path = immutable_dir.join(format!("{chunk_name}.chunk"));
    let secondary_path = immutable_dir.join(format!("{chunk_name}.secondary"));

    let offsets = read_secondary_offsets(&secondary_path)?;
    if offsets.is_empty() {
        return Ok(Vec::new());
    }

    let mut chunk_file = fs::File::open(&chunk_path)?;
    let chunk_len = chunk_file.metadata()?.len();
    let mut blocks = Vec::new();

    for (idx, start) in offsets.iter().copied().enumerate() {
        let end = offsets.get(idx + 1).copied().unwrap_or(chunk_len);
        if end < start {
            return Err(format!(
                "invalid immutable offsets in {} at index {idx}: start={start}, end={end}",
                secondary_path.display()
            )
            .into());
        }
        let block_len = end - start;
        if block_len == 0 {
            continue;
        }
        chunk_file.seek(SeekFrom::Start(start))?;
        let mut raw_block = vec![0u8; block_len as usize];
        chunk_file.read_exact(&mut raw_block)?;
        let Ok(header_cbor_slice) = extract_block_header_cbor(&raw_block) else {
            continue;
        };
        let hash = Hasher::<256>::hash(header_cbor_slice);
        let header_cbor = header_cbor_slice.to_vec();
        blocks.push(ImmutableBlock { hash, header_cbor, raw_block });
    }

    Ok(blocks)
}
