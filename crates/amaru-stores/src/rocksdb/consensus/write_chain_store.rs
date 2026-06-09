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

use amaru_kernel::{BlockHeader, HeaderHash, IsHeader, ORIGIN_HASH, Point, RawBlock, size::HEADER, to_cbor};
use amaru_observability::trace_span;
use amaru_ouroboros_traits::{Nonces, StoreError, WriteChainStore};
use rocksdb::{IteratorMode, PrefixRange, ReadOptions};

use crate::rocksdb::consensus::{
    RocksDBStore,
    util::{ANCHOR_PREFIX, BEST_CHAIN_PREFIX, BLOCK_PREFIX, CHAIN_PREFIX, CHILD_PREFIX, HEADER_PREFIX, NONCES_PREFIX},
};

impl WriteChainStore for RocksDBStore {
    fn store_header(&self, header: &BlockHeader) -> Result<(), StoreError> {
        let _span = trace_span!(
            amaru_observability::amaru::stores::consensus::STORE_HEADER,
            hash = header.hash(),
            db_system_name = "rocksdb".to_string(),
            db_operation_name = "put".to_string(),
            db_collection_name = "header".to_string()
        );
        let _guard = _span.enter();

        let hash = header.hash();
        let parent_hash = header.parent().unwrap_or(ORIGIN_HASH);

        self.with_transaction(|tx| {
            tx.put([&CHILD_PREFIX[..], &parent_hash[..], &hash[..]].concat(), [])
                .map_err(|e| StoreError::WriteError { error: e.to_string() })?;
            tx.put([&HEADER_PREFIX[..], &hash[..]].concat(), to_cbor(header))
                .map_err(|e| StoreError::WriteError { error: e.to_string() })?;
            Ok(())
        })
    }

    fn set_anchor_hash(&self, hash: &HeaderHash) -> Result<(), StoreError> {
        self.db.put(ANCHOR_PREFIX, hash.as_ref()).map_err(|e| StoreError::WriteError { error: e.to_string() })
    }

    fn set_best_chain_hash(&self, hash: &HeaderHash) -> Result<(), StoreError> {
        self.db.put(BEST_CHAIN_PREFIX, hash.as_ref()).map_err(|e| StoreError::WriteError { error: e.to_string() })
    }

    fn store_block(&self, hash: &HeaderHash, block: &RawBlock) -> Result<(), StoreError> {
        let _span = trace_span!(
            amaru_observability::amaru::stores::consensus::STORE_BLOCK,
            hash = *hash,
            db_system_name = "rocksdb".to_string(),
            db_operation_name = "put".to_string(),
            db_collection_name = "block".to_string()
        );
        let _guard = _span.enter();

        self.db
            .put([&BLOCK_PREFIX[..], &hash[..]].concat(), block.as_ref())
            .map_err(|e| StoreError::WriteError { error: e.to_string() })
    }

    fn set_block_valid(&self, hash: &HeaderHash, valid: bool) -> Result<(), StoreError> {
        self.db
            .put([&HEADER_PREFIX[..], &hash[..], &[0]].concat(), [valid as u8])
            .map_err(|e| StoreError::WriteError { error: e.to_string() })
    }

    fn put_nonces(&self, header: &HeaderHash, nonces: &Nonces) -> Result<(), StoreError> {
        self.db
            .put([&NONCES_PREFIX[..], &header[..]].concat(), to_cbor(nonces))
            .map_err(|e| StoreError::WriteError { error: e.to_string() })
    }

    fn switch_to_fork(&self, fork_point: &Point, forward_points: &[Point]) -> Result<(), StoreError> {
        let last = forward_points.last().unwrap_or(fork_point);
        let _span = trace_span!(
            amaru_observability::amaru::stores::consensus::SWITCH_TO_FORK,
            hash = last.hash(),
            slot = u64::from(last.slot_or_default()),
            db_system_name = "rocksdb".to_string(),
            db_operation_name = "delete".to_string(),
            db_collection_name = "chain".to_string()
        );
        let _guard = _span.enter();

        let fork_slot = u64::from(fork_point.slot_or_default()).to_be_bytes();
        let fork_key = [&CHAIN_PREFIX[..], &fork_slot[..]].concat();

        let slot = (u64::from(fork_point.slot_or_default()) + 1).to_be_bytes();
        let mut opts = ReadOptions::default();
        opts.set_iterate_range(PrefixRange(&CHAIN_PREFIX[..]));
        let starting_point = [&CHAIN_PREFIX[..], &slot[..]].concat();
        let mode = IteratorMode::From(starting_point.as_slice(), rocksdb::Direction::Forward);

        self.with_transaction(|tx| {
            // Validate the fork point *inside* the transaction using `get_for_update`, so any
            // concurrent writer that deletes or overwrites the fork-point chain entry causes
            // this transaction to conflict on commit rather than silently succeeding against
            // stale state.
            let existing =
                tx.get_for_update(&fork_key, true).map_err(|e| StoreError::ReadError { error: e.to_string() })?;
            let matches = existing
                .as_ref()
                .map(|bytes| bytes.len() == HEADER && bytes.as_slice() == fork_point.hash().as_ref())
                .unwrap_or(false);
            if !matches {
                return Err(StoreError::ReadError {
                    error: format!(
                        "Cannot switch to a fork from point {:?} as it does not exist on the best chain",
                        fork_point
                    ),
                });
            }

            let keys_to_delete: Vec<_> = tx
                .iterator_opt(mode, opts)
                .map(|kv| kv.map(|(key, _)| key).map_err(|e| StoreError::ReadError { error: e.to_string() }))
                .collect::<Result<_, _>>()?;

            for key in keys_to_delete {
                tx.delete(key).map_err(|e| StoreError::WriteError { error: e.to_string() })?;
            }

            for point in forward_points.iter() {
                let slot = u64::from(point.slot_or_default()).to_be_bytes();
                tx.put([&CHAIN_PREFIX[..], &slot[..]].concat(), point.hash().as_ref())
                    .map_err(|e| StoreError::WriteError { error: e.to_string() })?;
            }

            tx.put(BEST_CHAIN_PREFIX, forward_points.last().unwrap_or(fork_point).hash().as_ref())
                .map_err(|e| StoreError::WriteError { error: e.to_string() })?;

            Ok(())
        })
    }

    fn roll_forward_chain(&self, point: &Point) -> Result<(), StoreError> {
        let _span = trace_span!(
            amaru_observability::amaru::stores::consensus::ROLL_FORWARD_CHAIN,
            hash = point.hash(),
            slot = u64::from(point.slot_or_default()),
            db_system_name = "rocksdb".to_string(),
            db_operation_name = "put".to_string(),
            db_collection_name = "chain".to_string()
        );
        let _guard = _span.enter();

        self.with_transaction(|tx| {
            let slot = u64::from(point.slot_or_default()).to_be_bytes();
            tx.put([&CHAIN_PREFIX[..], &slot[..]].concat(), point.hash().as_ref())
                .map_err(|e| StoreError::WriteError { error: e.to_string() })?;
            tx.put(BEST_CHAIN_PREFIX, point.hash().as_ref())
                .map_err(|e| StoreError::WriteError { error: e.to_string() })?;
            Ok(())
        })
    }
}
