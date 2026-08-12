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

use std::collections::BTreeMap;

use amaru_kernel::{
    Hash, Header, HeaderHash, IsHeader, ORIGIN_HASH, Point, PoolId, RawBlock, Slot, Tip, from_cbor, size, size::HEADER,
};
use amaru_ouroboros_traits::{BaseReadChainStore, Nonces, StoreError};
use rocksdb::{Direction, IteratorMode, PrefixRange, ReadOptions};

use crate::rocksdb::consensus::{
    DbOps, OPCERT_PREFIX, RocksDBStore,
    util::{
        ANCHOR_PREFIX, BEST_CHAIN_PREFIX, BLOCK_PREFIX, CHAIN_PREFIX, CHILD_PREFIX, CONSENSUS_PREFIX_LEN,
        HEADER_PREFIX, NONCES_PREFIX,
    },
};

impl<T> BaseReadChainStore for RocksDBStore<T>
where
    T: DbOps + Send + Sync,
{
    fn load_header(&self, hash: &HeaderHash) -> Option<Header> {
        let prefix = [&HEADER_PREFIX[..], &hash[..]].concat();
        self.db.get_pinned(&prefix, ReadOptions::default()).ok().flatten().and_then(|bytes| from_cbor(bytes.as_ref()))
    }

    fn load_header_with_validity(&self, hash: &HeaderHash) -> Option<(Header, Option<bool>)> {
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
            .and_then(|bytes| from_cbor::<Header>(bytes.as_ref()))
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

    /// Return the latest opcert sequence number for the given pool id, and header we wish to validate.
    fn get_latest_opcert_sequence_number(&self, pool_id: &PoolId, header: &Header) -> Result<Option<u64>, StoreError> {
        let Some(parent) = header.parent() else {
            return Ok(None); // no previous header referencing an opcert sequence number
        };
        let Some(as_of_slot) = self.load_header(&parent).map(|h| h.slot()) else {
            return Ok(None);
        };
        let anchor_slot = self.load_header(&self.get_anchor_hash()).map(|h| h.slot()).unwrap_or(Slot::from(0));

        let prefix = [&OPCERT_PREFIX[..], &pool_id[..]].concat();

        // 1. Collect candidate entries in the volatile zone: slot in (anchor_slot, as_of_slot]
        let mut candidates: BTreeMap<HeaderHash, u64> = BTreeMap::new();

        // `floor` is the minimum slot of the candidates we have seen, which is used to limit the search in the next step.
        let mut floor = as_of_slot;
        if as_of_slot > anchor_slot {
            let start = [&prefix[..], &(u64::from(anchor_slot) + 1).to_be_bytes()[..]].concat();
            let mut opts = ReadOptions::default();
            opts.set_iterate_range(PrefixRange(prefix.as_slice()));
            for item in self.db.iterator_opt(IteratorMode::From(&start, Direction::Forward), opts) {
                let (key, value) = item.map_err(|e| StoreError::ReadError { error: e.to_string() })?;
                let (slot, hash) = decode_opcert_key(&key)?;
                if slot > as_of_slot {
                    break;
                }
                floor = floor.min(slot);
                if let Some(sequence_number) = from_cbor(&value) {
                    candidates.insert(hash, sequence_number);
                }
            }
        }

        // 2. Resolve candidates against the actual lineage of `as_of`
        if !candidates.is_empty() {
            let mut current = parent;
            loop {
                if let Some(sequence_number) = candidates.get(&current) {
                    return Ok(Some(*sequence_number));
                }
                let Some(header) = self.load_header(&current) else { break };
                if header.slot() <= floor {
                    break;
                }
                match header.parent() {
                    Some(parent) => current = parent,
                    None => break,
                }
            }
        }

        // 3. Immutable fallback: get the newest entry at slot <= min(anchor, as_of)
        // that sits on the best chain.
        // Note: the case where as_of < anchor would only exist if we try to revalidate an old header
        // when ingesting old blocks for example.
        let bound = anchor_slot.min(as_of_slot);
        let seek = [&prefix[..], &u64::from(bound).to_be_bytes()[..], &[0xff; 32][..]].concat();
        let mut opts = ReadOptions::default();
        opts.set_iterate_range(PrefixRange(prefix.as_slice()));
        for item in self.db.iterator_opt(IteratorMode::From(&seek, Direction::Reverse), opts) {
            let (key, value) = item.map_err(|e| StoreError::ReadError { error: e.to_string() })?;
            let (slot, hash) = decode_opcert_key(&key)?;
            if self.load_from_best_chain(&Point::Specific(slot, hash)).is_some() {
                return Ok(from_cbor(&value));
            }
        }
        Ok(None)
    }

    fn has_header(&self, hash: &HeaderHash) -> bool {
        let prefix = [&HEADER_PREFIX[..], &hash[..]].concat();
        self.db.get_pinned(&prefix, ReadOptions::default()).map(|opt| opt.is_some()).unwrap_or(false)
    }
}

/// Decode a slot || header_hash key used to store the opcert sequence numbers
pub(crate) fn decode_opcert_key(key: &[u8]) -> Result<(Slot, HeaderHash), StoreError> {
    let slot_start = CONSENSUS_PREFIX_LEN + size::POOL_COLD_KEY;
    let hash_start = slot_start + 8;
    let slot_bytes: [u8; 8] = key
        .get(slot_start..hash_start)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| StoreError::ReadError { error: "malformed opcert key".into() })?;
    let hash = key
        .get(hash_start..)
        .filter(|h| h.len() == HEADER)
        .ok_or_else(|| StoreError::ReadError { error: "malformed opcert key".into() })?;
    Ok((Slot::from(u64::from_be_bytes(slot_bytes)), Hash::from(hash)))
}

pub(crate) fn opcert_key(header: &Header) -> Vec<u8> {
    let slot = u64::from(header.slot()).to_be_bytes();
    [&OPCERT_PREFIX[..], &header.pool_id()[..], &slot[..], &header.hash()[..]].concat()
}

/// Return the next slot to look for when iterating over the best chain starting from the given point.
/// If the point is Origin, the slot is 0 by definition.
fn next_best_chain_start_slot(point: &Point) -> u64 {
    match point {
        Point::Specific(slot, _) => u64::from(*slot) + 1,
        Point::Origin => 0,
    }
}
