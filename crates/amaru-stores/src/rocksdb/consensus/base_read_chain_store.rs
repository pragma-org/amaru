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

use amaru_kernel::{BlockHeader, Hash, HeaderHash, ORIGIN_HASH, Point, RawBlock, Tip, from_cbor, size::HEADER};
use amaru_ouroboros_traits::{BaseReadChainStore, Nonces, StoreError};
use rocksdb::{IteratorMode, PrefixRange, ReadOptions};

use crate::rocksdb::consensus::{
    DbOps, RocksDBStore,
    util::{
        ANCHOR_PREFIX, BEST_CHAIN_PREFIX, BLOCK_PREFIX, CHAIN_PREFIX, CHILD_PREFIX, CONSENSUS_PREFIX_LEN,
        HEADER_PREFIX, NONCES_PREFIX,
    },
};

impl<T> BaseReadChainStore for RocksDBStore<T>
where
    T: DbOps + Send + Sync,
{
    fn load_header(&self, hash: &HeaderHash) -> Option<BlockHeader> {
        let prefix = [&HEADER_PREFIX[..], &hash[..]].concat();
        self.db.get_pinned(&prefix, ReadOptions::default()).ok().flatten().and_then(|bytes| from_cbor(bytes.as_ref()))
    }

    fn load_header_with_validity(&self, hash: &HeaderHash) -> Option<(BlockHeader, Option<bool>)> {
        let prefix = [&HEADER_PREFIX[..], &hash[..], &[0]].concat();
        let head_len = prefix.len() - 1;
        let mut results = self.db.multi_get(&[&prefix[..head_len], &prefix], ReadOptions::default()).into_iter();
        let header = results.next().and_then(|bytes| from_cbor(bytes.ok()??.as_ref()));
        let validity = results.next().and_then(|bytes| {
            let bytes = bytes.ok()??;
            if bytes.len() == 1 { Some(bytes[0] == 1) } else { None }
        });
        header.map(|h| (h, validity))
    }

    fn get_children(&self, hash: &HeaderHash) -> Vec<HeaderHash> {
        let mut result = Vec::new();
        let mut opts = ReadOptions::default();
        opts.set_iterate_range(PrefixRange([&CHILD_PREFIX[..], &hash[..]].concat()));

        for res in self.db.iterator_opt(IteratorMode::Start, opts) {
            #[expect(clippy::expect_used)]
            let (key, _value) = res.expect("error iterating over children");
            let mut arr = [0u8; HEADER];
            arr.copy_from_slice(&key[(CONSENSUS_PREFIX_LEN + HEADER)..]);
            result.push(Hash::from(arr));
        }
        result
    }

    fn get_anchor_hash(&self) -> HeaderHash {
        self.db
            .get_pinned(&ANCHOR_PREFIX, ReadOptions::default())
            .ok()
            .flatten()
            .and_then(|bytes| if bytes.len() == HEADER { Some(Hash::from(bytes.as_ref())) } else { None })
            .unwrap_or(ORIGIN_HASH)
    }

    fn get_anchor_tip(&self) -> Tip {
        let anchor_hash = self.get_anchor_hash();
        if anchor_hash == ORIGIN_HASH {
            return Tip::origin();
        }
        self.db
            .get_pinned(&[&HEADER_PREFIX[..], &anchor_hash[..]].concat(), ReadOptions::default())
            .ok()
            .flatten()
            .and_then(|bytes| from_cbor::<BlockHeader>(bytes.as_ref()))
            .map(|h| h.tip())
            .unwrap_or_else(Tip::origin)
    }

    fn get_best_chain_hash(&self) -> HeaderHash {
        self.db
            .get_pinned(&BEST_CHAIN_PREFIX, ReadOptions::default())
            .ok()
            .flatten()
            .and_then(|bytes| if bytes.len() == HEADER { Some(Hash::from(bytes.as_ref())) } else { None })
            .unwrap_or(ORIGIN_HASH)
    }

    fn load_from_best_chain(&self, point: &Point) -> Option<HeaderHash> {
        let slot = u64::from(point.slot_or_default()).to_be_bytes();
        self.db.get_pinned(&[&CHAIN_PREFIX[..], &slot[..]].concat(), ReadOptions::default()).ok().flatten().and_then(
            |bytes| {
                if bytes.len() == HEADER {
                    let hash = Hash::from(bytes.as_ref());
                    if *hash == *point.hash() { Some(hash) } else { None }
                } else {
                    None
                }
            },
        )
    }

    fn next_best_chain(&self, point: &Point) -> Option<Point> {
        let mut readopts = ReadOptions::default();
        readopts.set_iterate_range(PrefixRange(CHAIN_PREFIX));
        let slot = next_best_chain_start_slot(point);
        let prefix = [&CHAIN_PREFIX[..], &slot.to_be_bytes()].concat();
        let mut iter = self.db.iterator_opt(IteratorMode::From(&prefix, rocksdb::Direction::Forward), readopts);

        if let Some(Ok((k, v))) = iter.next() {
            #[expect(clippy::unwrap_used)]
            let slot_bytes: [u8; 8] = k[CHAIN_PREFIX.len()..CHAIN_PREFIX.len() + 8].try_into().unwrap();
            let slot = u64::from_be_bytes(slot_bytes);
            if v.len() == HEADER {
                let hash = <HeaderHash>::from(v.as_ref());
                Some(Point::Specific(slot.into(), hash))
            } else {
                None
            }
        } else {
            None
        }
    }

    fn load_block(&self, hash: &HeaderHash) -> Result<Option<RawBlock>, StoreError> {
        Ok(self
            .db
            .get_pinned(&[&BLOCK_PREFIX[..], &hash[..]].concat(), ReadOptions::default())?
            .map(|bytes| bytes.as_ref().into()))
    }

    fn has_block(&self, hash: &HeaderHash) -> Result<bool, StoreError> {
        let prefix = [&BLOCK_PREFIX[..], &hash[..]].concat();
        self.db.get_pinned(&prefix, ReadOptions::default()).map(|opt| opt.is_some())
    }

    fn get_nonces(&self, header: &HeaderHash) -> Option<Nonces> {
        self.db
            .get_pinned(&[&NONCES_PREFIX[..], &header[..]].concat(), ReadOptions::default())
            .ok()
            .flatten()
            .as_deref()
            .and_then(from_cbor)
    }

    fn has_header(&self, hash: &HeaderHash) -> bool {
        let prefix = [&HEADER_PREFIX[..], &hash[..]].concat();
        self.db.get_pinned(&prefix, ReadOptions::default()).map(|opt| opt.is_some()).unwrap_or(false)
    }
}

/// Return the next slot to look for when iterating over the best chain starting from the given point.
/// If the point is Origin, the slot is 0 by definition.
fn next_best_chain_start_slot(point: &Point) -> u64 {
    match point {
        Point::Specific(slot, _) => u64::from(*slot) + 1,
        Point::Origin => 0,
    }
}
