// Copyright 2025 PRAGMA
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

use crate::rocksdb::{
    RocksDbConfig,
    consensus::util::{
        ANCHOR_PREFIX, BEST_CHAIN_PREFIX, BLOCK_PREFIX, CHAIN_PREFIX, CHILD_PREFIX,
        CONSENSUS_PREFIX_LEN, HEADER_PREFIX, NONCES_PREFIX, open_db, open_or_create_db,
    },
};
use amaru_kernel::{
    BlockHeader, Hash, HeaderHash, IsHeader, ORIGIN_HASH, Point, RawBlock, cbor, from_cbor,
    size::HEADER, to_cbor,
};
use amaru_observability::trace;
use amaru_ouroboros_traits::{
    ChainStore, DiagnosticChainStore, Nonces, ReadOnlyChainStore, StoreError,
};
use rocksdb::{DB, IteratorMode, OptimisticTransactionDB, Options, PrefixRange, ReadOptions};
use std::{fs, path::PathBuf};

pub mod migration;
pub mod util;

pub use migration::*;

pub struct RocksDBStore {
    pub basedir: PathBuf,
    pub db: OptimisticTransactionDB,
}

pub struct ReadOnlyChainDB {
    db: DB,
}

impl RocksDBStore {
    /// Open an existing `RocksDBStore` with given configuration.
    ///
    /// This function will fail if:
    /// * the DB does not exist
    /// * the DB exists but with an incompatible version
    pub fn open(config: &RocksDbConfig) -> Result<Self, StoreError> {
        let (basedir, db) = open_db(config)?;

        check_db_version(&db)?;

        Ok(Self { db, basedir })
    }

    /// Create a `RocksDBStore` with given configuration.
    /// If the database already exists, an error will be raised.
    /// To check the existence of the database we only check the directory pointed at
    /// contains at least one file.
    /// NOTE: There should be a better way to detect whether or not a directory contains
    /// a RocksDB database.
    pub fn create(config: RocksDbConfig) -> Result<Self, StoreError> {
        let basedir = config.dir.clone();
        let list = fs::read_dir(&basedir);
        if let Ok(entries) = list
            && entries.count() > 0
        {
            return Err(StoreError::OpenError {
                error: format!(
                    "Cannot create RocksDB at {}, directory is not empty",
                    basedir.display()
                ),
            });
        }

        let (_, db) = open_or_create_db(&config)?;
        set_version(&db)?;

        Ok(Self { db, basedir })
    }

    /// Open or create a `RocksDBStore` with given configuration.
    ///
    /// This function is deemed "unsafe" because it automatically tries to migrate the
    /// DB it opens or creates which can potentially causes data corruption.
    pub fn open_and_migrate(config: &RocksDbConfig) -> Result<Self, StoreError> {
        let (basedir, db) = open_or_create_db(config)?;

        migrate_db(&db)?;

        Ok(Self { db, basedir })
    }

    pub fn open_for_readonly(config: &RocksDbConfig) -> Result<ReadOnlyChainDB, StoreError> {
        let basedir = config.dir.clone();
        let opts: Options = config.into();
        DB::open_for_read_only(&opts, basedir, false)
            .map_err(|e| StoreError::OpenError {
                error: e.to_string(),
            })
            .map(|db| ReadOnlyChainDB { db })
    }

    pub fn create_transaction(&self) -> rocksdb::Transaction<'_, OptimisticTransactionDB> {
        self.db.transaction()
    }

    /// Runs the provided closure within a transaction.
    ///
    /// The transaction is committed if the closure returns `Ok`, otherwise it is rolled back.
    /// Note the `commit` itself can also fail which is reported as a `StoreError::WriteError`.
    pub fn with_transaction<R, F>(&self, f: F) -> Result<R, StoreError>
    where
        F: FnOnce(&rocksdb::Transaction<'_, OptimisticTransactionDB>) -> Result<R, StoreError>,
    {
        let tx = self.db.transaction();
        match f(&tx) {
            Ok(result) => {
                tx.commit().map_err(|e| StoreError::WriteError {
                    error: e.to_string(),
                })?;
                Ok(result)
            }
            Err(err) => Err(err),
        }
    }
}

pub(crate) fn store_chain_point(
    db: &OptimisticTransactionDB,
    point: &Point,
) -> Result<(), StoreError> {
    let slot = u64::from(point.slot_or_default()).to_be_bytes();
    db.put(
        [&CHAIN_PREFIX[..], &slot[..]].concat(),
        point.hash().as_ref(),
    )
    .map_err(|e| StoreError::WriteError {
        error: e.to_string(),
    })
}

macro_rules! impl_ReadOnlyChainStore {
    (for $($s:ty),+) => {
        $(impl<H: IsHeader + Clone + for<'d> cbor::Decode<'d, ()>> ReadOnlyChainStore<H> for $s {
            fn load_header(&self, hash: &HeaderHash) -> Option<H> {
                let prefix = [&HEADER_PREFIX[..], &hash[..]].concat();
                self.db
                    .get_pinned(prefix)
                    .ok()
                    .and_then(|bytes| from_cbor(bytes?.as_ref()))
            }


            fn get_children(&self, hash: &HeaderHash) -> Vec<HeaderHash> {
                let mut result = Vec::new();
                let mut opts = ReadOptions::default();
                opts.set_iterate_range(PrefixRange([&CHILD_PREFIX[..], &hash[..]].concat()));

                for res in self.db.iterator_opt(IteratorMode::Start, opts) {
                    match res {
                        Ok((key, _value)) => {
                            let mut arr = [0u8; HEADER];
                            arr.copy_from_slice(&key[(CONSENSUS_PREFIX_LEN + HEADER)..]);
                            result.push(Hash::from(arr));
                        }
                        Err(err) => panic!("error iterating over children: {}", err),
                    }
                }
                result
            }

            fn get_anchor_hash(&self) -> HeaderHash {
                self.db
                    .get_pinned(&ANCHOR_PREFIX)
                    .ok()
                    .flatten()
                    .and_then(|bytes| {
                        if bytes.len() == HEADER {
                            Some(Hash::from(bytes.as_ref()))
                        } else {
                            None
                        }
                    })
                    .unwrap_or(ORIGIN_HASH)
            }

            fn get_best_chain_hash(&self) -> HeaderHash {
                self.db
                    .get_pinned(&BEST_CHAIN_PREFIX)
                    .ok()
                    .flatten()
                    .and_then(|bytes| {
                        if bytes.len() == HEADER {
                            Some(Hash::from(bytes.as_ref()))
                        } else {
                            None
                        }
                    })
                    .unwrap_or(ORIGIN_HASH)
            }

            fn has_header(&self, hash: &HeaderHash) -> bool {
                let prefix = [&HEADER_PREFIX[..], &hash[..]].concat();
                self.db.get_pinned(prefix)
                    .map(|opt| opt.is_some())
                    .unwrap_or(false)
            }

            fn get_nonces(&self, header: &HeaderHash) -> Option<Nonces> {
                self.db
                    .get_pinned([&NONCES_PREFIX[..], &header[..]].concat())
                    .ok()
                    .flatten()
                    .as_deref()
                    .and_then(from_cbor)
            }

            fn load_block(&self, hash: &HeaderHash) -> Result<Option<RawBlock>, StoreError> {
                Ok(self.db
                    .get_pinned([&BLOCK_PREFIX[..], &hash[..]].concat())
                    .map_err(|e| StoreError::ReadError {
                        error: e.to_string(),
                    })?
                    .map(|bytes| bytes.as_ref().into()))
            }

            fn load_from_best_chain(&self, point: &Point) -> Option<HeaderHash> {
                let slot = u64::from(point.slot_or_default()).to_be_bytes();
                self.db
                    .get_pinned([&CHAIN_PREFIX[..], &slot[..]].concat())
                    .ok()
                    .flatten()
                    .and_then(|bytes| {
                        if bytes.len() == HEADER {
                            let hash = Hash::from(bytes.as_ref());
                            if *hash == *point.hash() {
                                Some(hash)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    })
            }

            fn next_best_chain(&self, point: &Point) -> Option<Point> {
                let readopts = ReadOptions::default();
                let prefix = [&CHAIN_PREFIX[..], &(u64::from(point.slot_or_default()) + 1).to_be_bytes()].concat();
                let mut iter = self.db.iterator_opt(IteratorMode::From(&prefix, rocksdb::Direction::Forward), readopts);

                if let Some(Ok((k, v))) = iter.next() {
                    let slot_bytes = &k[CHAIN_PREFIX.len()..CHAIN_PREFIX.len() + 8];
                    let slot = u64::from_be_bytes(slot_bytes.try_into().unwrap());
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

        })*
    }
}

impl_ReadOnlyChainStore!(for ReadOnlyChainDB, RocksDBStore);

impl DiagnosticChainStore for ReadOnlyChainDB {
    #[allow(clippy::panic)]
    fn load_headers(&self) -> Box<dyn Iterator<Item = BlockHeader> + '_> {
        Box::new(
            self.db
                .prefix_iterator(HEADER_PREFIX)
                .filter_map(|item| match item {
                    Ok((_k, v)) => from_cbor(v.as_ref()),
                    Err(err) => panic!("error iterating over headers: {}", err),
                }),
        )
    }

    #[allow(clippy::panic)]
    fn load_nonces(&self) -> Box<dyn Iterator<Item = (HeaderHash, Nonces)> + '_> {
        Box::new(
            self.db
                .prefix_iterator(NONCES_PREFIX)
                .filter_map(|item| match item {
                    Ok((k, v)) => {
                        let hash = Hash::from(&k[CONSENSUS_PREFIX_LEN..]);
                        from_cbor(&v).map(|nonces| (hash, nonces))
                    }
                    Err(err) => panic!("error iterating over nonces: {}", err),
                }),
        )
    }

    #[allow(clippy::panic)]
    fn load_blocks(&self) -> Box<dyn Iterator<Item = (HeaderHash, RawBlock)> + '_> {
        let mut opts = ReadOptions::default();
        opts.set_iterate_range(PrefixRange(&BLOCK_PREFIX[..]));
        Box::new(
            self.db
                .iterator_opt(IteratorMode::Start, opts)
                .map(|item| match item {
                    Ok((k, v)) => {
                        let hash = Hash::from(&k[CONSENSUS_PREFIX_LEN..]);
                        (hash, RawBlock::from(v))
                    }
                    Err(err) => panic!("error iterating over blocks: {}", err),
                }),
        )
    }

    #[allow(clippy::expect_used)]
    fn load_parents_children(
        &self,
    ) -> Box<dyn Iterator<Item = (HeaderHash, Vec<HeaderHash>)> + '_> {
        let mut groups: Vec<(HeaderHash, Vec<HeaderHash>)> = Vec::new();
        let mut current_parent: Option<HeaderHash> = None;
        let mut current_children: Vec<HeaderHash> = Vec::new();
        let mut opts = ReadOptions::default();
        opts.set_iterate_range(PrefixRange(&CHILD_PREFIX[..]));

        for kv in self.db.iterator_opt(IteratorMode::Start, opts) {
            let (k, _v) = kv.expect("error iterating over children keys");

            //Key layout: [CHILD_PREFIX][parent][child]
            let parent_start = CONSENSUS_PREFIX_LEN;
            let parent_end = parent_start + HEADER;
            let child_start = parent_end;
            let child_end = child_start + HEADER;

            let mut parent_arr = [0u8; HEADER];
            parent_arr.copy_from_slice(&k[parent_start..parent_end]);
            let parent_hash = Hash::from(parent_arr);

            let mut child_arr = [0u8; HEADER];

            child_arr.copy_from_slice(&k[child_start..child_end]);
            let child_hash = Hash::from(child_arr);

            match &current_parent {
                Some(p) if p == &parent_hash => {
                    current_children.push(child_hash);
                }
                Some(prev_parent) => {
                    groups.push((*prev_parent, std::mem::take(&mut current_children)));
                    current_parent = Some(parent_hash);
                    current_children.push(child_hash);
                }
                None => {
                    current_parent = Some(parent_hash);
                    current_children.push(child_hash);
                }
            }
        }

        if let Some(p) = current_parent {
            groups.push((p, current_children));
        }

        Box::new(groups.into_iter())
    }
}

use std::fmt::Debug;
impl<H: IsHeader + Clone + Debug + for<'d> cbor::Decode<'d, ()>> ChainStore<H> for RocksDBStore {
    #[trace(amaru::stores::consensus::STORE_HEADER, hash = format!("{}", header.hash()))]
    fn store_header(&self, header: &H) -> Result<(), StoreError> {
        let hash = header.hash();
        let tx = self.db.transaction();
        if let Some(parent) = header.parent() {
            tx.put([&CHILD_PREFIX[..], &parent[..], &hash[..]].concat(), [])
                .map_err(|e| StoreError::WriteError {
                    error: e.to_string(),
                })?;
        };
        tx.put([&HEADER_PREFIX[..], &hash[..]].concat(), to_cbor(header))
            .map_err(|e| StoreError::WriteError {
                error: e.to_string(),
            })?;
        tx.commit().map_err(|e| StoreError::WriteError {
            error: e.to_string(),
        })
    }

    fn put_nonces(&self, header: &HeaderHash, nonces: &Nonces) -> Result<(), StoreError> {
        self.db
            .put([&NONCES_PREFIX[..], &header[..]].concat(), to_cbor(nonces))
            .map_err(|e| StoreError::WriteError {
                error: e.to_string(),
            })
    }

    #[trace(amaru::stores::consensus::STORE_BLOCK, hash = format!("{}", hash))]
    fn store_block(&self, hash: &HeaderHash, block: &RawBlock) -> Result<(), StoreError> {
        self.db
            .put([&BLOCK_PREFIX[..], &hash[..]].concat(), block.as_ref())
            .map_err(|e| StoreError::WriteError {
                error: e.to_string(),
            })
    }

    fn set_anchor_hash(&self, hash: &HeaderHash) -> Result<(), StoreError> {
        self.db
            .put(ANCHOR_PREFIX, hash.as_ref())
            .map_err(|e| StoreError::WriteError {
                error: e.to_string(),
            })
    }

    fn set_best_chain_hash(&self, hash: &HeaderHash) -> Result<(), StoreError> {
        self.db
            .put(BEST_CHAIN_PREFIX, hash.as_ref())
            .map_err(|e| StoreError::WriteError {
                error: e.to_string(),
            })
    }

    #[trace(amaru::stores::consensus::ROLL_FORWARD_CHAIN,
        hash = format!("{}", point.hash()),
        slot = u64::from(point.slot_or_default()))]
    fn roll_forward_chain(&self, point: &Point) -> Result<(), StoreError> {
        store_chain_point(&self.db, point)
    }

    #[trace(amaru::stores::consensus::ROLLBACK_CHAIN,
        hash = format!("{}", point.hash()),
        slot = u64::from(point.slot_or_default()))]
    fn rollback_chain(&self, point: &Point) -> Result<usize, StoreError> {
        if <Self as ReadOnlyChainStore<BlockHeader>>::load_from_best_chain(self, point).is_none() {
            return Err(StoreError::ReadError {
                error: format!(
                    "Cannot roll back chain to point {:?} as it does not exist on the best chain",
                    point
                ),
            });
        };

        // keep the rollback point so delete starting 1 slot after it
        let slot = (u64::from(point.slot_or_default()) + 1).to_be_bytes();
        let mut count = 0usize;
        let mut opts = ReadOptions::default();
        opts.set_iterate_range(PrefixRange(&CHAIN_PREFIX[..]));
        let starting_point = [&CHAIN_PREFIX[..], &slot[..]].concat();
        let mode = IteratorMode::From(starting_point.as_slice(), rocksdb::Direction::Forward);

        self.with_transaction(|tx| {
            for kv in tx.iterator_opt(mode, opts) {
                match kv {
                    Ok((k, _v)) => {
                        tx.delete(k).map_err(|e| StoreError::WriteError {
                            error: e.to_string(),
                        })?;
                        count += 1;
                    }
                    Err(e) => {
                        return Err(StoreError::ReadError {
                            error: e.to_string(),
                        });
                    }
                }
            }

            Ok(count)
        })
    }
}

#[cfg(test)]
pub mod test {
    use super::*;
    use crate::rocksdb::consensus::{migration::migrate_db_path, util::CHAIN_DB_VERSION};
    use amaru_kernel::{
        BlockHeader, Nonce, ORIGIN_HASH, any_header_hash, any_header_with_parent,
        any_headers_chain, make_header,
        utils::tests::{random_bytes, run_strategy},
    };
    use amaru_ouroboros_traits::{
        ChainStore, DiagnosticChainStore, in_memory_consensus_store::InMemConsensusStore,
    };
    use rocksdb::Direction;
    use std::{collections::BTreeMap, fs, io, path::Path, sync::Arc};

    #[test]
    fn both_rw_and_ro_can_be_open_on_same_dir() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path();
        let _rw_store = initialise_test_rw_store(path);
        if let Err(e) = initialise_test_ro_store(path) {
            panic!("failed to re-open DB in read-only mode: {}", e);
        }
    }

    #[test]
    fn rocksdb_chain_store_can_get_header_it_puts() {
        with_db(|db| {
            let header = BlockHeader::from(make_header(1, 0, None));
            db.store_header(&header).unwrap();
            let header2 = db.load_header(&header.hash()).unwrap();
            assert_eq!(header, header2);
        })
    }

    #[test]
    fn rocksdb_chain_store_can_get_block_it_puts() {
        with_db(|db| {
            let hash: HeaderHash = random_bytes(32).as_slice().into();
            let block = RawBlock::from(&*vec![1; 64]);

            db.store_block(&hash, &block).unwrap();
            let block2 = db.load_block(&hash).unwrap();
            assert_eq!(Some(block), block2);
        })
    }

    #[test]
    fn rocksdb_chain_store_returns_not_found_for_nonexistent_block() {
        with_db(|db| {
            let nonexistent_hash: HeaderHash = random_bytes(HEADER).as_slice().into();
            let result = db.load_block(&nonexistent_hash).unwrap();

            assert_eq!(result, None);
        });
    }

    #[test]
    fn best_chain_hash_when_store_is_empty() {
        with_db(|db| assert_eq!(db.get_best_chain_hash(), ORIGIN_HASH))
    }

    #[test]
    fn store_best_chain_hash() {
        with_db(|db| {
            let best_chain = run_strategy(any_header_hash());
            db.set_best_chain_hash(&best_chain).unwrap();
            assert_eq!(db.get_best_chain_hash(), best_chain);
        })
    }

    #[test]
    fn anchor_hash_when_store_is_empty() {
        with_db(|db| {
            assert_eq!(db.get_anchor_hash(), ORIGIN_HASH);
        })
    }

    #[test]
    fn store_anchor_hash() {
        with_db(|db| {
            let anchor = run_strategy(any_header_hash());
            db.set_anchor_hash(&anchor).unwrap();
            assert_eq!(db.get_anchor_hash(), anchor);
        })
    }

    #[test]
    fn store_parent_children_relationship_for_header() {
        with_db(|db| {
            // h0 -> h1 -> h2
            //      \
            //       -> h3
            let mut chain = run_strategy(any_headers_chain(3));
            let h3 = run_strategy(any_header_with_parent(chain[1].hash()));
            chain.push(h3.clone());

            for header in &chain {
                db.store_header(header).unwrap();
            }

            let mut children = db.get_children(&chain[1].hash());
            children.sort();
            let mut expected = vec![chain[2].hash(), h3.hash()];
            expected.sort();
            assert_eq!(children, expected);
        })
    }

    #[test]
    fn load_all_headers() {
        with_db_path(|(db, path)| {
            let mut headers: Vec<BlockHeader> = vec![];
            for i in 0..10usize {
                let parent = if i == 0 {
                    None
                } else {
                    Some(headers[i - 1].hash())
                };
                let header = make_header(i as u64, i as u64 * 10, parent).into();
                db.store_header(&header).unwrap();
                headers.push(header);
            }
            headers.sort();

            let db = initialise_test_ro_store(path).unwrap();

            let mut result: Vec<BlockHeader> = db.load_headers().collect();
            result.sort();
            assert_eq!(result, headers);
        })
    }

    #[test]
    fn load_parents_children() {
        with_db_path(|(db, path)| {
            // h0 -> h1 -> h2
            //      \
            //       -> h3 -> h4
            let mut chain = run_strategy(any_headers_chain(3));
            let h3 = run_strategy(any_header_with_parent(chain[1].hash()));
            chain.push(h3.clone());
            let h4 = run_strategy(any_header_with_parent(h3.hash()));
            chain.push(h4);

            let mut expected = BTreeMap::new();

            for header in &chain {
                if let Some(parent) = header.parent() {
                    expected
                        .entry(parent)
                        .or_insert_with(Vec::new)
                        .push(header.hash());
                }
                db.store_header(header).unwrap();
            }

            let db = initialise_test_ro_store(path).unwrap();

            let result = sort_entries(db.load_parents_children().collect::<Vec<_>>());
            let expected = sort_entries(expected.into_iter().collect::<Vec<_>>());
            assert_eq!(result, expected);
        })
    }

    #[test]
    fn load_nonces() {
        with_db_path(|(db, path)| {
            let chain = run_strategy(any_headers_chain(3));
            let mut expected = BTreeMap::new();
            for header in &chain {
                let nonces = Nonces {
                    active: Nonce::from(random_bytes(32).as_slice()),
                    evolving: Nonce::from(random_bytes(32).as_slice()),
                    candidate: Nonce::from(random_bytes(32).as_slice()),
                    tail: header.parent().unwrap_or(ORIGIN_HASH),
                    epoch: Default::default(),
                };
                db.put_nonces(&header.hash(), &nonces).unwrap();
                expected.insert(header.hash(), nonces);
            }

            let db = initialise_test_ro_store(path).unwrap();

            let mut result = db.load_nonces().collect::<Vec<_>>();
            result.sort();
            let mut expected = expected.into_iter().collect::<Vec<_>>();
            expected.sort();
            assert_eq!(result, expected);
        })
    }

    #[test]
    fn load_blocks() {
        with_db_path(|(db, path)| {
            let chain = run_strategy(any_headers_chain(3));
            let mut expected = BTreeMap::new();
            for header in &chain {
                let block = RawBlock::from(random_bytes(32).as_slice());
                db.store_block(&header.hash(), &block).unwrap();
                expected.insert(header.hash(), block);
            }

            let db = initialise_test_ro_store(path).unwrap();

            let mut result = db.load_blocks().collect::<Vec<_>>();
            result.sort();
            let mut expected = expected.into_iter().collect::<Vec<_>>();
            expected.sort();
            assert_eq!(result, expected);
        })
    }

    #[test]
    fn retrieve_best_chain() {
        with_db(|db| {
            // create a chain and store it as the best chain
            // with its anchor and tip.
            let chain = run_strategy(any_headers_chain(15));
            for header in &chain {
                db.store_header(header).unwrap();
            }

            // set the chain anchor to the 5th header
            // and the tip to the last header
            db.set_anchor_hash(&chain[4].hash()).unwrap();
            db.set_best_chain_hash(&chain[14].hash()).unwrap();
            let result = db.retrieve_best_chain();
            assert_eq!(
                result,
                chain[4..].iter().map(|h| h.hash()).collect::<Vec<_>>()
            );
        })
    }

    #[test]
    fn update_best_chain_to_block_slot_given_new_block_is_valid() {
        with_db(|store| {
            let chain = populate_db(store.clone());
            let new_tip = run_strategy(any_header_with_parent(chain[9].hash()));

            store
                .roll_forward_chain(&new_tip.point())
                .expect("should roll forward successfully");

            assert_eq!(
                store.load_from_best_chain(&new_tip.point()),
                Some(new_tip.hash())
            );
        });
    }

    #[test]
    fn update_best_chain_to_rollback_point() {
        with_db(|store| {
            let chain = populate_db(store.clone());

            store
                .rollback_chain(&chain[5].point())
                .expect("should rollback successfully");

            assert_eq!(store.load_from_best_chain(&chain[9].point()), None);
            assert_eq!(
                store.load_from_best_chain(&chain[5].point()),
                Some(chain[5].hash())
            );
        });
    }

    #[test]
    fn next_best_chain_returns_successor_give_valid_point() {
        with_db(|store| {
            let chain = populate_db(store.clone());

            let result = store
                .next_best_chain(&chain[5].point())
                .expect("should find successor");

            assert_eq!(result, chain[6].point());
        });
    }

    #[test]
    fn next_best_chain_returns_first_point_on_chain_given_origin() {
        with_db(|store| {
            let chain = populate_db(store.clone());

            let result = store
                .next_best_chain(&Point::Origin)
                .expect("should find successor");

            assert_eq!(result, chain[0].point());
        });
    }

    #[test]
    fn next_best_chain_returns_none_given_point_is_not_on_chain() {
        with_db(|store| {
            let _chain = populate_db(store.clone());
            let invalid_point = Point::Specific(100.into(), run_strategy(any_header_hash()));

            assert!(store.next_best_chain(&invalid_point).is_none());
        });
    }

    #[test]
    fn next_best_chain_returns_none_given_point_is_tip() {
        with_db(|store| {
            let _chain = populate_db(store.clone());
            let tip = store.get_best_chain_hash();
            let tip_header = store.load_header(&tip).unwrap();

            assert!(store.next_best_chain(&tip_header.point()).is_none());
        });
    }

    #[test]
    fn raises_error_if_rollback_is_not_on_best_chain() {
        with_db(|store| {
            let chain = populate_db(store.clone());
            let new_tip = run_strategy(any_header_with_parent(chain[6].hash()));

            let result = store.rollback_chain(&new_tip.point());

            if result.is_ok() {
                panic!("expected test to fail");
            }
        });
    }

    // MIGRATIONS

    #[test]
    fn fails_to_open_rw_db_if_it_does_not_exist() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path();
        let basedir = init_dir(path);
        let config = RocksDbConfig::new(basedir);

        let result = RocksDBStore::open(&config);
        match result {
            Err(StoreError::OpenError { .. }) => (), // OK
            Err(e) => panic!("Expected OpenError error, got: {:?}", e),
            _other => panic!("Expected failure to open RocksDBStore but it succeeded"),
        }
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn fails_to_open_rw_db_if_it_exists_with_wrong_version() {
        let tempdir = tempfile::tempdir().unwrap();
        let target = tempdir.path();
        let config = RocksDbConfig::new(target.to_path_buf());
        let source = PathBuf::from("sample-chain-db/v0");

        copy_recursively(source, target).unwrap();

        let result = RocksDBStore::open(&config);
        match result {
            Err(StoreError::IncompatibleChainStoreVersions { stored, current }) => {
                assert_eq!(stored, 0);
                assert_eq!(current, CHAIN_DB_VERSION);
            }
            Err(e) => panic!("Expected OpenError error, got: {:?}", e),
            _other => panic!("Expected failure to open RocksDBStore but it succeeded"),
        }
    }

    #[test]
    fn raises_an_error_when_creating_a_database_given_directory_is_non_empty() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path();
        let basedir = init_dir(path);

        fs::write(basedir.join("some_file.txt"), b"some data").unwrap();
        let result = RocksDBStore::create(RocksDbConfig::new(basedir));

        match result {
            Err(StoreError::OpenError { error: _ }) => {
                // expected
            }
            Err(e) => panic!("Expected OpenError, got: {:?}", e),
            _other => panic!("Expected failure to create DB but it succeeded"),
        };
    }

    #[test]
    fn creates_a_database_with_current_version_given_directory_is_empty() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path();
        let basedir = init_dir(path);

        let store = RocksDBStore::create(RocksDbConfig::new(basedir))
            .expect("should create DB successfully");
        let version = get_version(&store.db).expect("should read version successfully");

        assert_eq!(version, CHAIN_DB_VERSION);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn can_convert_v0_sample_db_to_v1() {
        let tempdir = tempfile::tempdir().unwrap();
        let target = tempdir.path();
        let config = RocksDbConfig::new(target.to_path_buf());
        let source = PathBuf::from("sample-chain-db/v0");

        copy_recursively(source, target).unwrap();

        let (_, db) = open_db(&config).expect("cannot open sample v0 DB");
        migrate_to_v1(&db).expect("Migration should succeed");

        let header: Option<BlockHeader> = db
            .get_pinned(
                [
                    &HEADER_PREFIX[..],
                    hex::decode(SAMPLE_HASH).unwrap().as_slice(),
                ]
                .concat(),
            )
            .ok()
            .and_then(|bytes| from_cbor(bytes?.as_ref()));

        assert!(header.is_some(), "Sample data should be preserved");
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn can_convert_v1_sample_db_to_v2() {
        use std::str::FromStr;

        let tempdir = tempfile::tempdir().unwrap();
        let target = tempdir.path();
        let config = RocksDbConfig::new(target.to_path_buf());
        let source = PathBuf::from("sample-chain-db/v1");

        copy_recursively(source, target).unwrap();

        let result = migrate_db_path(target).expect("Migration should succeed");

        let db = RocksDBStore::open(&config)
            .expect("DB should successfully be opened as it's been migrated");
        assert_eq!((1, 2), result);
        let header: Option<HeaderHash> =
            <RocksDBStore as ReadOnlyChainStore<BlockHeader>>::load_from_best_chain(
                &db,
                &Point::Specific(5.into(), Hash::from_str(SAMPLE_HASH).unwrap()),
            );
        assert!(header.is_some(), "Sample data should be preserved");
    }

    #[test]
    fn migrate_db_fails_given_directory_does_not_exist() {
        let tempdir = tempfile::tempdir().unwrap();
        let target = tempdir.path();

        let result = migrate_db_path(target);

        match result {
            Err(StoreError::OpenError { error: _ }) => {
                // expected
            }
            Err(e) => panic!("Expected OpenError, got: {:?}", e),
            _other => panic!("Expected failure to migrate DB but it succeeded"),
        }
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn open_or_create_succeeds_given_directory_exists() {
        let tempdir = tempfile::tempdir().unwrap();
        let target = tempdir.path();
        let config = RocksDbConfig::new(target.to_path_buf());
        let source = PathBuf::from("sample-chain-db/v0");

        copy_recursively(source, target).unwrap();

        let store = RocksDBStore::open_and_migrate(&config).expect("should create DB successfully");
        let version = get_version(&store.db).expect("should read version successfully");

        assert_eq!(version, CHAIN_DB_VERSION);
    }

    #[test]
    fn iterator_over_chain() {
        let tempdir = tempfile::tempdir().unwrap();
        let target = tempdir.path();
        let config = RocksDbConfig::new(target.to_path_buf());
        let (_, db) = open_or_create_db(&config).expect("should open DB successfully");

        // populate DB
        for slot in 1..10 {
            let prefix = [&CHAIN_PREFIX[..], &(slot as u64).to_be_bytes()[..]].concat();
            let header_hash = run_strategy(any_header_hash());
            db.put(&prefix, header_hash)
                .expect("should put data successfully");
        }
        // iterate over chain from 4 to 8
        let slot4 = 4u64.to_be_bytes();
        let slot6 = 6u64.to_be_bytes();
        let slot7 = 7u64.to_be_bytes();
        let slot8 = 8u64.to_be_bytes();
        let slot9 = 9u64.to_be_bytes();
        let slot10 = 10u64.to_be_bytes();
        let prefix = [&CHAIN_PREFIX[..], &slot4].concat();

        let mut readopts = ReadOptions::default();
        readopts.set_iterate_upper_bound([&CHAIN_PREFIX[..], &slot10[..]].concat());
        let mut iter = db.iterator_opt(IteratorMode::From(&prefix, Direction::Forward), readopts);
        let mut count = 0;
        while let Some(Ok((_, v))) = iter.next()
            && count < 3
        {
            let _header_hash: HeaderHash = Hash::from(v.as_ref());
            count += 1;
        }

        // we can delete keys the iterator has seen and not seen
        db.delete([&CHAIN_PREFIX[..], &slot6].concat())
            .expect("should delete data successfully");
        db.delete([&CHAIN_PREFIX[..], &slot7].concat())
            .expect("should delete data successfully");

        // iterator continues from where it left off, skipping deleted keys
        assert_eq!(
            *(iter.next().unwrap().unwrap().0),
            [&CHAIN_PREFIX[..], &slot8].concat()
        );
        assert_eq!(
            *(iter.next().unwrap().unwrap().0),
            [&CHAIN_PREFIX[..], &slot9].concat()
        );
    }

    // HELPERS

    const SAMPLE_HASH: &str = "4b1f95026700f5b3df8432b3f93b023f3cbdf13c85704e0f71b0089e6e81c947";

    fn populate_db(store: Arc<dyn ChainStore<BlockHeader>>) -> Vec<BlockHeader> {
        let chain = run_strategy(any_headers_chain(10));

        // Set the anchor to the first header in the chain
        store
            .set_anchor_hash(&chain[0].hash())
            .expect("should set anchor hash successfully");

        for header in chain.iter() {
            store
                .set_best_chain_hash(&header.hash())
                .expect("should set best chain hash successfully");
            store
                .roll_forward_chain(&header.point())
                .expect("should roll forward successfully");
            store
                .store_header(header)
                .expect("should store header successfully");
        }
        chain
    }

    pub fn initialise_test_rw_store(path: &std::path::Path) -> RocksDBStore {
        let basedir = init_dir(path);
        let config = RocksDbConfig::new(basedir);

        RocksDBStore::create(config).expect("fail to initialise RocksDB")
    }

    pub fn initialise_test_ro_store(path: &std::path::Path) -> Result<ReadOnlyChainDB, StoreError> {
        let basedir = init_dir(path);
        RocksDBStore::open_for_readonly(&RocksDbConfig::new(basedir))
    }

    fn init_dir(path: &std::path::Path) -> PathBuf {
        let basedir = path.join("rocksdb_chain_store");
        use std::fs::create_dir_all;
        create_dir_all(&basedir).expect("fail to create test dir");
        basedir
    }

    // creates a sample db at the given path, populating with some data
    // this function exists to make it easy to test DB migration:
    //
    // 1. create a sample DB with the old version
    // 2. run migration code
    // 3. check that the data is still there and correct
    #[expect(dead_code)]
    fn create_sample_db(path: &Path) {
        let db = initialise_test_rw_store(path);
        let chain = run_strategy(any_headers_chain(10));
        for header in &chain {
            let block = RawBlock::from(random_bytes(32).as_slice());
            db.store_header(header).unwrap();
            <RocksDBStore as ChainStore<BlockHeader>>::store_block(&db, &header.hash(), &block)
                .unwrap();
            let nonces = Nonces {
                active: Nonce::from(random_bytes(32).as_slice()),
                evolving: Nonce::from(random_bytes(32).as_slice()),
                candidate: Nonce::from(random_bytes(32).as_slice()),
                tail: header.parent().unwrap_or(ORIGIN_HASH),
                epoch: Default::default(),
            };
            <RocksDBStore as ChainStore<BlockHeader>>::put_nonces(&db, &header.hash(), &nonces)
                .unwrap();
        }
        <RocksDBStore as ChainStore<BlockHeader>>::set_anchor_hash(&db, &chain[1].hash()).unwrap();
        <RocksDBStore as ChainStore<BlockHeader>>::set_best_chain_hash(&db, &chain[9].hash())
            .unwrap();
    }

    fn with_db(f: impl Fn(Arc<dyn ChainStore<BlockHeader>>)) {
        // try first with in-memory store
        let in_memory_store: Arc<dyn ChainStore<BlockHeader>> =
            Arc::new(InMemConsensusStore::new());
        f(in_memory_store);

        // then with rocksdb store
        let tempdir = tempfile::tempdir().unwrap();
        let rw_store: Arc<dyn ChainStore<BlockHeader>> =
            Arc::new(initialise_test_rw_store(tempdir.path()));
        f(rw_store);
    }

    fn with_db_path(f: impl Fn((Arc<dyn ChainStore<BlockHeader>>, &Path))) {
        let tempdir = tempfile::tempdir().unwrap();
        let rw_store: Arc<dyn ChainStore<BlockHeader>> =
            Arc::new(initialise_test_rw_store(tempdir.path()));
        f((rw_store, tempdir.path()));
    }

    fn sort_entries(
        mut v: Vec<(HeaderHash, Vec<HeaderHash>)>,
    ) -> Vec<(HeaderHash, Vec<HeaderHash>)> {
        v.sort_by_key(|(k, _)| *k);
        for (_, children) in &mut v {
            children.sort();
        }
        v
    }

    // from https://nick.groenen.me/notes/recursively-copy-files-in-rust/
    // NOTE: the stored database is only valid for Unix (Linux/MacOS) systems, so
    // any test relying on it should be guarded for not running on windows
    pub fn copy_recursively(
        source: impl AsRef<Path>,
        destination: impl AsRef<Path>,
    ) -> io::Result<()> {
        fs::create_dir_all(&destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            let filetype = entry.file_type()?;
            if filetype.is_dir() {
                copy_recursively(entry.path(), destination.as_ref().join(entry.file_name()))?;
            } else {
                fs::copy(entry.path(), destination.as_ref().join(entry.file_name()))?;
            }
        }
        Ok(())
    }
}
