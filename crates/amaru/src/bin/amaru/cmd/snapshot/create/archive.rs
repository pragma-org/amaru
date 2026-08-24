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
};

use tar::{Builder, Header};

use super::{EpochTarget, PACKAGED_BLOCKS_FILE_NAME};

pub(super) fn archive_path_for_target(snapshot_root: &Path, target: &EpochTarget) -> PathBuf {
    snapshot_root.join(format!("{}.{}.tar.zst", target.slot, target.hash))
}

pub(super) fn write_snapshot_archive(
    snapshot_dir: &Path,
    archive_path: &Path,
    target: &EpochTarget,
    packaged_blocks: &[u8],
) -> anyhow::Result<()> {
    let tmp_path = archive_path.with_extension("tmp");
    let file = fs::File::create(&tmp_path)?;
    build_snapshot_archive(snapshot_dir, target, packaged_blocks, io::BufWriter::new(file))?;
    fs::rename(tmp_path, archive_path)?;
    Ok(())
}

fn build_snapshot_archive<W: io::Write>(
    snapshot_dir: &Path,
    target: &EpochTarget,
    packaged_blocks: &[u8],
    writer: W,
) -> anyhow::Result<()> {
    let root_name = format!("{}.{}", target.slot, target.hash);

    let encoder = zstd::Encoder::new(writer, 0)?;
    let mut tar = Builder::new(encoder);

    append_directory_entry(&mut tar, Path::new(&root_name))?;
    if snapshot_dir.join("tables").is_file() {
        append_directory_entry(&mut tar, &PathBuf::from(&root_name).join("tables"))?;
    }

    let mut entries = collect_directory_entries(snapshot_dir)?;
    entries.sort();

    for path in entries {
        let relative = path.strip_prefix(snapshot_dir)?;
        let metadata = fs::symlink_metadata(&path)?;
        let relative = if metadata.is_file() && relative == Path::new("tables") {
            PathBuf::from("tables").join("tvar")
        } else {
            relative.to_path_buf()
        };
        if relative == Path::new(PACKAGED_BLOCKS_FILE_NAME) {
            anyhow::bail!("snapshot source contains reserved entry {}", path.display());
        }
        let archive_path = PathBuf::from(&root_name).join(relative);

        if metadata.is_dir() {
            append_directory_entry(&mut tar, &archive_path)?;
        } else if metadata.is_file() {
            append_file_entry(&mut tar, &archive_path, &path)?;
        } else {
            anyhow::bail!("unsupported snapshot entry {}", path.display());
        }
    }

    append_bytes_entry(&mut tar, &PathBuf::from(&root_name).join(PACKAGED_BLOCKS_FILE_NAME), packaged_blocks)?;

    tar.into_inner()?.finish()?.flush()?;
    Ok(())
}

fn walk_directory<F>(root: &Path, mut f: F) -> Result<(), io::Error>
where
    F: FnMut(&Path, &fs::FileType) -> io::Result<()>,
{
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                pending.push(path.clone());
            }
            f(&path, &file_type)?;
        }
    }
    Ok(())
}

fn collect_directory_entries(root: &Path) -> Result<Vec<PathBuf>, io::Error> {
    let mut entries = Vec::new();
    walk_directory(root, |path, _| {
        entries.push(path.to_path_buf());
        Ok(())
    })?;
    Ok(entries)
}

fn append_directory_entry<W: io::Write>(tar: &mut Builder<W>, archive_path: &Path) -> io::Result<()> {
    let mut header = Header::new_gnu();
    header.set_entry_type(tar::EntryType::Directory);
    header.set_mode(0o755);
    header.set_size(0);
    header.set_mtime(0);
    header.set_uid(0);
    header.set_gid(0);
    header.set_cksum();
    tar.append_data(&mut header, archive_path, io::empty())
}

fn append_file_entry<W: io::Write>(tar: &mut Builder<W>, archive_path: &Path, file_path: &Path) -> io::Result<()> {
    let mut file = fs::File::open(file_path)?;
    let size = file.metadata()?.len();
    append_entry(tar, archive_path, size, &mut file)
}

fn append_bytes_entry<W: io::Write>(tar: &mut Builder<W>, archive_path: &Path, bytes: &[u8]) -> io::Result<()> {
    append_entry(tar, archive_path, bytes.len() as u64, bytes)
}

fn append_entry<W: io::Write, R: io::Read>(
    tar: &mut Builder<W>,
    archive_path: &Path,
    size: u64,
    reader: R,
) -> io::Result<()> {
    let mut header = Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_mode(0o644);
    header.set_size(size);
    header.set_mtime(0);
    header.set_uid(0);
    header.set_gid(0);
    header.set_cksum();
    tar.append_data(&mut header, archive_path, reader)
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Read};

    use amaru::bootstrap::validate_publishable_snapshot_archive;
    use amaru_kernel::{
        Epoch, Header, IsHeader, PREPROD_ERA_HISTORY, cardano::network_block::make_encoded_block,
        extract_block_header_cbor, from_cbor, make_header,
    };
    use tar::Archive;
    use tempfile::TempDir;
    use zstd::Decoder;

    use super::{EpochTarget, write_snapshot_archive};

    #[test]
    fn writes_archive_directly_from_db_analyser_snapshot() {
        let temp_dir = TempDir::new().unwrap();
        let source = temp_dir.path().join("42_db-analyser");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("state"), b"state").unwrap();
        fs::write(source.join("tables"), b"utxo").unwrap();
        fs::write(source.join("meta"), b"meta").unwrap();
        let block = make_encoded_block(&make_header(1, 42, None), &PREPROD_ERA_HISTORY).to_vec();
        let header: Header = from_cbor(extract_block_header_cbor(&block).unwrap()).unwrap();
        let target =
            EpochTarget { epoch: Epoch::from(1), slot: header.slot(), hash: header.hash(), parent_point: None };
        let archive_path = temp_dir.path().join("snapshot.tar.zst");
        let packaged_blocks = serde_json::to_vec(&vec![hex::encode(block)]).unwrap();

        write_snapshot_archive(&source, &archive_path, &target, &packaged_blocks).unwrap();
        validate_publishable_snapshot_archive(&archive_path, &header.point().to_network_point().to_string()).unwrap();

        let root = format!("{}.{}", target.slot, target.hash);
        let decoder = Decoder::new(fs::File::open(&archive_path).unwrap()).unwrap();
        let mut archive = Archive::new(decoder);
        let entries = archive
            .entries()
            .unwrap()
            .map(|entry| {
                let mut entry = entry.unwrap();
                let path = entry.path().unwrap().to_string_lossy().into_owned();
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).unwrap();
                (path, bytes)
            })
            .collect::<Vec<_>>();

        assert!(entries.contains(&(format!("{root}/state"), b"state".to_vec())));
        assert!(entries.contains(&(format!("{root}/tables/tvar"), b"utxo".to_vec())));
        assert!(entries.contains(&(format!("{root}/meta"), b"meta".to_vec())));
        assert!(entries.contains(&(format!("{root}/bootstrap.blocks.json"), packaged_blocks)));
        assert!(!temp_dir.path().join(&root).exists());
    }
}
