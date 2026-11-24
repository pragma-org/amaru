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
    EraName, Hash, Header, HeaderContent, HeaderHash, IsHeader, NetworkPoint, NetworkTip, Point, PoolId, RawBlock,
    Slot, from_cbor, size, size::HEADER,
};
use amaru_ouroboros_traits::{BaseReadChainStore, NextBestChainHeader, Nonces, StoreError, verify_header_integrity};
use rocksdb::{Direction, IteratorMode, PrefixRange, ReadOptions};

use crate::rocksdb::consensus::{
    DbOps, OPCERT_PREFIX, RocksDBStore,
    util::{
        ANCHOR_PREFIX, BEST_CHAIN_PREFIX, BLOCK_METADATA_PREFIX, BLOCK_PREFIX, CHAIN_PREFIX, CHILD_PREFIX,
        CONSENSUS_PREFIX_LEN, HEADER_PREFIX, HISTORICAL_BLOCK_PREFIX, NONCES_PREFIX, historical_block_key,
    },
};

impl<T> BaseReadChainStore for RocksDBStore<T>
where
    T: DbOps + Send + Sync,
{
    fn load_header(&self, hash: &HeaderHash) -> Option<Header> {
        let prefix = [&HEADER_PREFIX[..], &hash[..]].concat();
        self.db
            .get_pinned(&prefix, ReadOptions::default())
            .ok()
            .flatten()
            .and_then(|bytes| from_cbor(bytes.as_ref()))
            .map(|header| verify_header_integrity(header, hash))
    }

    fn load_header_with_validity(&self, hash: &HeaderHash) -> Option<(Header, Option<bool>)> {
        let prefix = [&HEADER_PREFIX[..], &hash[..], &[0]].concat();
        let head_len = prefix.len() - 1;
        let mut results = self.db.multi_get(&[&prefix[..head_len], &prefix], ReadOptions::default()).into_iter();
        let header = results
            .next()
            .and_then(|bytes| from_cbor(bytes.ok()??.as_ref()))
            .map(|header| verify_header_integrity(header, hash));
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

    fn get_anchor_point(&self) -> Point {
        self.db
            .get_pinned(&ANCHOR_PREFIX, ReadOptions::default())
            .ok()
            .flatten()
            .and_then(|bytes| from_cbor::<NetworkTip>(bytes.as_ref()).map(Point::from))
            .unwrap_or(Point::Origin)
    }

    fn get_best_chain_tip(&self) -> Point {
        self.db
            .get_pinned(&BEST_CHAIN_PREFIX, ReadOptions::default())
            .ok()
            .flatten()
            .and_then(|bytes| from_cbor::<NetworkTip>(bytes.as_ref()).map(Point::from))
            .unwrap_or(Point::Origin)
    }

    fn is_on_best_chain(&self, point: NetworkPoint) -> bool {
        match point {
            NetworkPoint::Origin => true,
            NetworkPoint::Specific(slot, hash) => {
                let slot = u64::from(slot).to_be_bytes();
                self.db
                    .get_pinned(&[&CHAIN_PREFIX[..], &slot[..]].concat(), ReadOptions::default())
                    .ok()
                    .flatten()
                    .and_then(|bytes| from_cbor::<NetworkTip>(bytes.as_ref()).map(Point::from))
                    .is_some_and(|stored| stored.hash() == hash)
            }
        }
    }

    fn next_best_chain(&self, point: &Point) -> Option<Point> {
        match point {
            Point::Origin => {
                let mut readopts = ReadOptions::default();
                readopts.set_iterate_range(PrefixRange(CHAIN_PREFIX));
                let prefix = [&CHAIN_PREFIX[..], &0u64.to_be_bytes()].concat();
                let mut iter = self.db.iterator_opt(IteratorMode::From(&prefix, rocksdb::Direction::Forward), readopts);
                iter.next()
                    .and_then(|kv| kv.ok())
                    .and_then(|(_k, v)| from_cbor::<NetworkTip>(v.as_ref()).map(Point::from))
            }
            Point::Specific(_, hash, _) => self.get_children(hash).into_iter().find_map(|child| {
                let header = self.load_header(&child)?;
                self.is_on_best_chain(header.point().into()).then_some(header.point())
            }),
        }
    }

    fn find_historical_intersect(&self, point: NetworkPoint) -> Option<Point> {
        self.historical_points_at(point.slot_or_default())
            .ok()?
            .iter()
            .any(|(stored, _)| *stored == point)
            .then(|| self.read_historical_header(point).ok().map(|(point, _)| point))?
    }

    fn next_historical_header(&self, pointer: &Point) -> Result<Option<NextBestChainHeader>, StoreError> {
        if *pointer != Point::Origin
            && !self
                .historical_points_at(pointer.slot_or_default())?
                .iter()
                .any(|(point, _)| *point == pointer.to_network_point())
        {
            return Ok(None);
        }
        let Some(next) = self.next_historical_point(pointer)? else {
            return if *pointer == Point::Origin {
                Ok(None)
            } else {
                Err(StoreError::ReadError { error: format!("missing historical successor of {pointer}") })
            };
        };
        let (point, header) = self.read_historical_header(next)?;
        Ok(Some(NextBestChainHeader::RollForward { point, header }))
    }

    fn load_block(&self, hash: &HeaderHash) -> Result<Option<RawBlock>, StoreError> {
        Ok(self
            .db
            .get_pinned(&[&BLOCK_PREFIX[..], &hash[..]].concat(), ReadOptions::default())?
            .map(|bytes| bytes.as_ref().into()))
    }

    fn load_block_metadata(&self, hash: &HeaderHash) -> Result<Option<(NetworkPoint, Option<HeaderHash>)>, StoreError> {
        if let Some((slot, parent)) = self.read_block_metadata(hash)? {
            return Ok(Some((NetworkPoint::Specific(slot, *hash), parent)));
        }
        if !self.has_block(hash)? {
            return Ok(None);
        }
        let header = match self.load_header(hash) {
            Some(header) => Some(header),
            None => self.load_block(hash)?.and_then(|block| block.decode_header().ok()),
        };
        Ok(header.map(|header| (header.point().into(), header.parent())))
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
        let anchor_slot = self.get_anchor_point().slot_or_default();

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
            let (_slot, hash) = decode_opcert_key(&key)?;
            if let Some(header) = self.load_header(&hash)
                && self.is_on_best_chain(header.point().into())
            {
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

impl<T: DbOps + Send + Sync> RocksDBStore<T> {
    /// Recover both a boundary block and a regular block when they share an indexed slot.
    fn historical_points_at(&self, slot: Slot) -> Result<Vec<(NetworkPoint, Option<HeaderHash>)>, StoreError> {
        let Some(bytes) = self.db.get_pinned(&historical_block_key(slot), ReadOptions::default())? else {
            return Ok(Vec::new());
        };
        let indexed: NetworkPoint = from_cbor(bytes.as_ref())
            .ok_or_else(|| StoreError::ReadError { error: "invalid historical block point".into() })?;
        if indexed.slot_or_default() != slot {
            return Err(StoreError::ReadError { error: "historical index slot mismatch".into() });
        }
        let mut hash = indexed.hash();
        let mut points = Vec::new();
        // A slot contains at most a Byron boundary block and its regular successor.
        for index in 0..=2 {
            let Some((stored_slot, parent)) = self.read_block_metadata(&hash)? else { break };
            if stored_slot != slot {
                break;
            }
            if index == 2 {
                return Err(StoreError::ReadError { error: "more than two historical blocks at one slot".into() });
            }
            points.push((NetworkPoint::Specific(slot, hash), parent));
            let Some(parent) = parent else { break };
            hash = parent;
        }
        points.reverse();
        Ok(points)
    }

    /// Seek the current and next indexed slots, ordering same-slot blocks by their parent links.
    fn next_historical_point(&self, point: &Point) -> Result<Option<NetworkPoint>, StoreError> {
        let mut opts = ReadOptions::default();
        opts.set_iterate_range(PrefixRange(HISTORICAL_BLOCK_PREFIX));
        let start = historical_block_key(point.slot_or_default());
        let parent = (*point != Point::Origin).then_some(point.hash());
        for entry in self.db.iterator_opt(IteratorMode::From(&start, Direction::Forward), opts).take(2) {
            let (_, bytes) = entry.map_err(|error| StoreError::ReadError { error: error.to_string() })?;
            let indexed: NetworkPoint = from_cbor(&bytes)
                .ok_or_else(|| StoreError::ReadError { error: "invalid historical block point".into() })?;
            for (candidate, previous) in self.historical_points_at(indexed.slot_or_default())? {
                if previous == parent {
                    return Ok(Some(candidate));
                }
            }
        }
        Ok(None)
    }

    /// Copy only the header out of the pinned block value, and read each block at most once.
    fn read_historical_header(&self, point: NetworkPoint) -> Result<(Point, HeaderContent), StoreError> {
        if let Some(header) = self.load_header(&point.hash()) {
            return Ok((header.point(), HeaderContent::new(&header, EraName::Conway)));
        }
        let key = [&BLOCK_PREFIX[..], &point.hash()[..]].concat();
        let bytes = self
            .db
            .get_pinned(&key, ReadOptions::default())?
            .ok_or_else(|| StoreError::ReadError { error: format!("missing historical block {point}") })?;
        let header = HeaderContent::from_block(bytes.as_ref())
            .map_err(|error| StoreError::ReadError { error: error.to_string() })?;
        let height = header.block_height().map_err(|error| StoreError::ReadError { error: error.to_string() })?;
        Ok((Point::Specific(point.slot_or_default(), point.hash(), height), header))
    }

    pub(super) fn read_block_metadata(
        &self,
        hash: &HeaderHash,
    ) -> Result<Option<(Slot, Option<HeaderHash>)>, StoreError> {
        self.db
            .get_pinned(&[&BLOCK_METADATA_PREFIX[..], &hash[..]].concat(), ReadOptions::default())?
            .map(|bytes| {
                from_cbor(bytes.as_ref())
                    .ok_or_else(|| StoreError::ReadError { error: format!("invalid block metadata for {hash}") })
            })
            .transpose()
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
    Ok((
        Slot::from(u64::from_be_bytes(slot_bytes)),
        Hash::try_from(hash).map_err(|e| StoreError::ReadError { error: e.to_string() })?,
    ))
}

pub(crate) fn opcert_key(header: &Header) -> Vec<u8> {
    let slot = u64::from(header.slot()).to_be_bytes();
    [&OPCERT_PREFIX[..], &header.pool_id()[..], &slot[..], &header.hash()[..]].concat()
}
