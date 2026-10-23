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

use std::{cell::Cell, num::NonZeroUsize};

use amaru_kernel::{
    EraHistory, IsHeader, NetworkPoint, Point,
    cardano::network_block::{EncodedTestBlock, make_encoded_chain},
    make_header,
};
use amaru_ouroboros_traits::{BlockRange, BlockReadError, BlockReadLimits, ReadChainStore, WriteChainStore};
use tempfile::TempDir;

use crate::rocksdb::{RocksDbConfig, consensus::RocksDBStore};

fn point(slot: u64, hash: u8) -> NetworkPoint {
    NetworkPoint::Specific(slot.into(), [hash; 32].into())
}

fn anchor(store: &RocksDBStore, point: NetworkPoint) {
    store.set_anchor_point(&Point::Specific(point.slot_or_default(), point.hash(), 42.into())).unwrap();
}

fn fixture() -> (TempDir, RocksDBStore, Vec<NetworkPoint>) {
    let directory = tempfile::tempdir().unwrap();
    let store = RocksDBStore::create(RocksDbConfig::new(directory.path().join("chain"))).unwrap();
    // The first two blocks share a slot, as Byron boundary and regular blocks can.
    let blocks = make_encoded_chain(
        [0, 0, 10, 20, 30].into_iter().enumerate().map(|(i, slot)| make_header(i as u64 + 1, slot, None)),
        &EraHistory::default(),
    );
    let points = blocks.iter().map(|block| block.header.point().to_network_point()).collect::<Vec<_>>();
    for block in &blocks {
        store.store_header(&block.header).unwrap();
        store.store_block(&block.header.hash(), &block.raw).unwrap();
        store.roll_forward_chain(&block.header.point()).unwrap();
    }
    anchor(&store, points[3]);
    (directory, store, points)
}

fn read_points(store: &dyn ReadChainStore, range: BlockRange) -> Result<Vec<NetworkPoint>, BlockReadError> {
    let expected = store.blocks(range)?.map(|entry| entry.map(|entry| entry.point)).collect::<Result<Vec<_>, _>>()?;
    let limits = BlockReadLimits { max_blocks: NonZeroUsize::MIN, ..BlockReadLimits::default() };
    let resolved = store.open_block_range(range, limits, &|| false)?;
    let mut after = resolved.after;
    let mut actual = Vec::new();
    loop {
        let batch = store.read_block_batch(
            BlockRange { after: Some(after), through: Some(resolved.through) },
            limits,
            &|| false,
        )?;
        actual.extend(batch.blocks.iter().map(|entry| entry.point));
        after = batch.last_point;
        if batch.complete {
            break;
        }
    }
    assert_eq!(actual, expected);
    Ok(actual)
}

#[test]
fn every_range_resumes_exclusively_and_stops_inclusively() {
    let (_directory, store, points) = fixture();
    for end in 0..4 {
        for start in 0..=end {
            assert_eq!(
                read_points(&store, BlockRange { after: Some(points[start]), through: Some(points[end]) }).unwrap(),
                points[start + 1..=end],
            );
        }
        assert_eq!(
            read_points(&store, BlockRange { after: None, through: Some(points[end]) }).unwrap(),
            points[..=end],
        );
    }
}

#[test]
fn rejects_unknown_wrong_slot_orphaned_and_volatile_endpoints() {
    let (_directory, store, points) = fixture();
    let block = EncodedTestBlock::from_seed(&make_header(3, 11, Some(points[1].hash())), &EraHistory::default());
    store.store_header(&block.header).unwrap();
    store.store_block(&block.header.hash(), &block.raw).unwrap();
    let orphan = block.header.point().to_network_point();
    let wrong_slot = NetworkPoint::Specific(11.into(), points[2].hash());
    for invalid in [point(10, 99), wrong_slot, orphan, points[4]] {
        assert!(matches!(
            store.blocks(BlockRange { after: Some(invalid), through: None }),
            Err(BlockReadError::InvalidPosition { .. })
        ));
        assert!(matches!(
            store.blocks(BlockRange { after: None, through: Some(invalid) }),
            Err(BlockReadError::InvalidPosition { .. })
        ));
        assert!(store.blocks(BlockRange { after: Some(invalid), through: Some(invalid) }).is_err());
    }
}

#[test]
fn snapshot_survives_fork_switch_pruning_and_anchor_changes() {
    let (_directory, store, points) = fixture();
    let mut reader = store.blocks(BlockRange::default()).unwrap();
    assert_eq!(reader.next().unwrap().unwrap().point, points[0]);

    let block = EncodedTestBlock::from_seed(&make_header(3, 21, Some(points[1].hash())), &EraHistory::default());
    store.store_header(&block.header).unwrap();
    store.store_block(&block.header.hash(), &block.raw).unwrap();
    let fork = block.header.point().to_network_point();
    anchor(&store, fork);
    store.set_best_chain_tip(&Point::Specific(fork.slot_or_default(), fork.hash(), 2.into())).unwrap();
    for point in &points[1..] {
        store.remove_header(&point.hash()).unwrap();
    }

    assert_eq!(reader.map(|entry| entry.unwrap().point).collect::<Vec<_>>(), points[1..4]);
}

#[test]
fn missing_body_yields_an_error_and_fuses_the_reader() {
    let (_directory, store, points) = fixture();
    // Preserve the header so the missing body is discovered lazily during iteration.
    store.remove_block(&points[1].hash()).unwrap();
    let mut reader = store.blocks(BlockRange::default()).unwrap();
    assert_eq!(reader.next().unwrap().unwrap().point, points[0]);
    assert!(matches!(
        reader.next(),
        Some(Err(BlockReadError::MissingHistory { hash })) if hash == points[1].hash()
    ));
    assert!(reader.next().is_none());
    assert!(reader.next().is_none());
}

#[test]
fn batches_capture_empty_boundaries_and_resume_without_pinning_history() {
    let (_directory, store, points) = fixture();
    let limits = BlockReadLimits { max_blocks: NonZeroUsize::MIN, ..BlockReadLimits::default() };
    let range = store.open_block_range(BlockRange::default(), limits, &|| false).unwrap();
    assert_eq!(range.through, points[3]);
    let mut after = range.after;
    for expected in &points[..4] {
        let batch = store
            .read_block_batch(BlockRange { after: Some(after), through: Some(range.through) }, limits, &|| false)
            .unwrap();
        assert_eq!(batch.blocks.len(), 1);
        assert_eq!(batch.last_point, *expected);
        assert_eq!(batch.complete, *expected == range.through);
        after = batch.last_point;
        // Advancing the node's anchor does not change the captured explicit endpoint.
        anchor(&store, points[4]);
    }
    let empty = store
        .read_block_batch(BlockRange { after: Some(after), through: Some(range.through) }, limits, &|| false)
        .unwrap();
    assert!(empty.complete);
    assert!(empty.blocks.is_empty());
    assert_eq!(empty.range.through, points[3]);
    store.remove_block(&points[2].hash()).unwrap();
    assert!(matches!(
        store.read_block_batch(BlockRange { after: Some(points[1]), through: Some(range.through) }, limits, &|| false),
        Err(BlockReadError::MissingHistory { .. })
    ));
    for invalid in [point(10, 99), NetworkPoint::Specific(11.into(), points[2].hash())] {
        assert!(
            store
                .open_block_range(BlockRange { after: Some(invalid), through: Some(range.through) }, limits, &|| false)
                .is_err()
        );
    }
}

#[test]
fn batch_opening_and_iteration_are_cancellable_and_work_bounded() {
    let (_directory, store, _) = fixture();
    let limits = BlockReadLimits::default();
    for cancel_at in 0..20 {
        let calls = Cell::new(0);
        let cancelled = || {
            let n = calls.get();
            calls.set(n + 1);
            n >= cancel_at
        };
        assert!(matches!(
            store.read_block_batch(BlockRange::default(), limits, &cancelled),
            Err(BlockReadError::Cancelled)
        ));
    }
    for cancel_at in 0..5 {
        let calls = Cell::new(0);
        let cancelled = || {
            let n = calls.get();
            calls.set(n + 1);
            n >= cancel_at
        };
        assert!(matches!(
            store.open_block_range(BlockRange::default(), limits, &cancelled),
            Err(BlockReadError::Cancelled)
        ));
    }
    let tiny = BlockReadLimits { max_steps: NonZeroUsize::MIN, ..limits };
    assert!(matches!(store.open_block_range(BlockRange::default(), tiny, &|| false), Err(BlockReadError::WorkLimit)));
    let bytes = BlockReadLimits { max_bytes: NonZeroUsize::MIN, ..limits };
    assert!(matches!(
        store.read_block_batch(BlockRange::default(), bytes, &|| false),
        Err(BlockReadError::BlockTooLarge { .. })
    ));
}
