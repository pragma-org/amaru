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

use amaru_kernel::{Header, HeaderHash, IsHeader, ORIGIN_HASH, Point, RawBlock, from_cbor, to_cbor};
use amaru_observability::debug_span;
use amaru_ouroboros_traits::{Nonces, OpcertSequenceNumbers, StoreError, WriteChainStore};
use rocksdb::{IteratorMode, PrefixRange, ReadOptions, WriteBatch};

use crate::rocksdb::consensus::{
    OPCERT_PREFIX, RocksDBStore,
    base_read_chain_store::opcert_key,
    util::{ANCHOR_PREFIX, BEST_CHAIN_PREFIX, BLOCK_PREFIX, CHAIN_PREFIX, CHILD_PREFIX, HEADER_PREFIX, NONCES_PREFIX},
};

impl WriteChainStore for RocksDBStore {
    fn store_header(&self, header: &Header) -> Result<(), StoreError> {
        let span = debug_span!(stores::consensus::header::STORE, hash = header.hash());
        let _guard = span.enter();

        self.with_batch(|batch| {
            put_header(batch, header);
            Ok(())
        })
    }

    fn store_validated_header(&self, header: &Header, nonces: &Nonces) -> Result<(), StoreError> {
        let span = debug_span!(stores::consensus::header::STORE, hash = header.hash());
        let _guard = span.enter();

        self.with_batch(|batch| {
            put_header(batch, header);
            batch.put([&NONCES_PREFIX[..], &header.hash()[..]].concat(), to_cbor(nonces));
            Ok(())
        })
    }

    fn set_anchor_point(&self, point: &Point) -> Result<(), StoreError> {
        self.db.put(ANCHOR_PREFIX, to_cbor(point)).map_err(|e| StoreError::WriteError { error: e.to_string() })
    }

    fn set_best_chain_tip(&self, tip: &Point) -> Result<(), StoreError> {
        self.db.put(BEST_CHAIN_PREFIX, to_cbor(tip)).map_err(|e| StoreError::WriteError { error: e.to_string() })
    }

    fn store_block(&self, hash: &HeaderHash, block: &RawBlock) -> Result<(), StoreError> {
        let span = debug_span!(stores::consensus::block::STORE, hash = *hash);
        let _guard = span.enter();

        self.db
            .put([&BLOCK_PREFIX[..], &hash[..]].concat(), block.as_ref())
            .map_err(|e| StoreError::WriteError { error: e.to_string() })
    }

    fn set_block_valid(&self, hash: &HeaderHash, valid: bool) -> Result<(), StoreError> {
        self.db
            .put([&HEADER_PREFIX[..], &hash[..], &[0]].concat(), [valid as u8])
            .map_err(|e| StoreError::WriteError { error: e.to_string() })
    }

    fn remove_block_valid(&self, hash: &HeaderHash) -> Result<(), StoreError> {
        self.db
            .delete([&HEADER_PREFIX[..], &hash[..], &[0]].concat())
            .map_err(|e| StoreError::WriteError { error: e.to_string() })
    }

    fn put_nonces(&self, header: &HeaderHash, nonces: &Nonces) -> Result<(), StoreError> {
        self.db
            .put([&NONCES_PREFIX[..], &header[..]].concat(), to_cbor(nonces))
            .map_err(|e| StoreError::WriteError { error: e.to_string() })
    }

    fn put_opcert_seed(&self, counters: &OpcertSequenceNumbers, at: &Point) -> Result<(), StoreError> {
        let slot = u64::from(at.slot_or_default()).to_be_bytes();
        let hash = at.hash();
        self.with_batch(|batch| {
            for (pool_id, sequence_number) in counters.iter() {
                batch.put([&OPCERT_PREFIX[..], &pool_id[..], &slot[..], &hash[..]].concat(), to_cbor(sequence_number));
            }
            Ok(())
        })
    }

    fn switch_to_fork(&self, fork_point: &Point, forward_points: &[Point]) -> Result<(), StoreError> {
        let last = forward_points.last().unwrap_or(fork_point);
        let span =
            debug_span!(stores::consensus::chain::SWITCH_TO_FORK, hash = last.hash(), slot = last.slot_or_default(),);
        let _guard = span.enter();

        let fork_slot = u64::from(fork_point.slot_or_default()).to_be_bytes();
        let fork_key = [&CHAIN_PREFIX[..], &fork_slot[..]].concat();

        // `adopt_chain` is the only writer of `CHAIN_PREFIX`/`BEST_CHAIN_PREFIX` and processes its
        // mailbox sequentially, so the read here and the subsequent batch write below cannot race.
        let existing = self.db.get_pinned(&fork_key).map_err(|e| StoreError::ReadError { error: e.to_string() })?;
        let matches = existing
            .as_ref()
            .and_then(|bytes| from_cbor::<Point>(bytes.as_ref()))
            .is_some_and(|stored| stored.hash() == fork_point.hash());
        if !matches {
            return Err(StoreError::ReadError {
                error: format!(
                    "Cannot switch to a fork from point {:?} as it does not exist on the best chain",
                    fork_point
                ),
            });
        }

        let slot = (u64::from(fork_point.slot_or_default()) + 1).to_be_bytes();
        let mut opts = ReadOptions::default();
        opts.set_iterate_range(PrefixRange(&CHAIN_PREFIX[..]));
        let starting_point = [&CHAIN_PREFIX[..], &slot[..]].concat();
        let mode = IteratorMode::From(starting_point.as_slice(), rocksdb::Direction::Forward);

        let keys_to_delete: Vec<_> = self
            .db
            .iterator_opt(mode, opts)
            .map(|kv| kv.map(|(key, _)| key).map_err(|e| StoreError::ReadError { error: e.to_string() }))
            .collect::<Result<_, _>>()?;

        self.with_batch(|batch| {
            for key in keys_to_delete {
                batch.delete(key);
            }

            for point in forward_points.iter() {
                let slot = u64::from(point.slot_or_default()).to_be_bytes();
                batch.put([&CHAIN_PREFIX[..], &slot[..]].concat(), to_cbor(point));
            }

            batch.put(BEST_CHAIN_PREFIX, to_cbor(forward_points.last().unwrap_or(fork_point)));

            Ok(())
        })
    }

    fn roll_forward_chain(&self, point: &Point) -> Result<(), StoreError> {
        let span =
            debug_span!(stores::consensus::chain::ROLL_FORWARD, hash = point.hash(), slot = point.slot_or_default(),);
        let _guard = span.enter();

        self.with_batch(|batch| {
            let slot = u64::from(point.slot_or_default()).to_be_bytes();
            batch.put([&CHAIN_PREFIX[..], &slot[..]].concat(), to_cbor(point));
            batch.put(BEST_CHAIN_PREFIX, to_cbor(point));
            Ok(())
        })
    }
}

/// Record a header, its link to its parent, and the opcert sequence number it declares. Every
/// stored header must contribute to the opcert index, otherwise the sequence numbers observed on
/// the chain go stale and subsequent headers get rejected as being too far ahead.
fn put_header(batch: &mut WriteBatch, header: &Header) {
    let hash = header.hash();
    let parent_hash = header.parent().unwrap_or(ORIGIN_HASH);

    batch.put([&CHILD_PREFIX[..], &parent_hash[..], &hash[..]].concat(), []);
    batch.put([&HEADER_PREFIX[..], &hash[..]].concat(), to_cbor(header));
    batch.put(opcert_key(header), to_cbor(&header.op_cert_seq()));
}
