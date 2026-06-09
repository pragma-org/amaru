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
    collections::BTreeMap,
    fmt::{Display, Formatter},
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use amaru_kernel::{
    BlockHeader, BlockHeight, Hash, HeaderHash, IsHeader, NonEmptyVec, Nonce, ORIGIN_HASH, Point, RawBlock, Slot, Tip,
    any_header_hash, any_header_with_parent, any_headers_chain, from_cbor, make_header,
    size::HEADER,
    utils::tests::{random_bytes, run_strategy},
};
use amaru_ouroboros_traits::{
    BaseReadChainStore, ChainStore, ChildTipsMode, DiagnosticChainStore, FindAncestorOnBestChainResult,
    FindCommonAncestorResult, FullChainStore, MissingBlocks, MissingBlocksResult, NextBestChainHeader, Nonces,
    SampleAncestorPointsResult, StoreError, in_memory_chain_store::InMemoryChainStore,
};
use rocksdb::{DB, Direction, IteratorMode, ReadOptions};

use super::*;
use crate::rocksdb::{
    RocksDbConfig,
    consensus::{migration::migrate_db_path, util::CHAIN_DB_VERSION},
};

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
fn rocksdb_chain_store_can_check_if_block_exists() {
    with_db(|db| {
        let hash: HeaderHash = random_bytes(32).as_slice().into();
        let block = RawBlock::from(&*vec![1; 64]);

        assert!(!db.has_block(&hash).unwrap());
        db.store_block(&hash, &block).unwrap();
        assert!(db.has_block(&hash).unwrap());
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
fn anchor_tip_when_store_is_empty() {
    with_db(|db| {
        assert_eq!(db.get_anchor_tip(), Tip::origin());
    })
}

#[test]
fn anchor_tip_returns_origin_when_anchor_header_is_not_stored() {
    with_db(|db| {
        let anchor = run_strategy(any_header_hash());
        db.set_anchor_hash(&anchor).unwrap();
        assert_eq!(db.get_anchor_tip(), Tip::origin());
    })
}

#[test]
fn anchor_tip_returns_tip_of_stored_anchor_header() {
    with_db(|db| {
        let header = BlockHeader::from(make_header(1, 0, None));
        db.store_header(&header).unwrap();
        db.set_anchor_hash(&header.hash()).unwrap();
        assert_eq!(db.get_anchor_tip(), header.tip());
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
fn store_parent_children_relationship_for_first_header() {
    with_db(|db| {
        // ORIGIN_HASH -> h0
        let chain = run_strategy(any_headers_chain(1));
        db.store_header(&chain[0]).unwrap();

        let children = db.get_children(&ORIGIN_HASH);
        let expected = vec![chain[0].hash()];
        assert_eq!(children, expected);
    })
}

#[test]
fn load_all_headers() {
    with_db_path(|(db, path)| {
        let mut headers: Vec<BlockHeader> = vec![];
        for i in 0..10usize {
            let parent = if i == 0 { None } else { Some(headers[i - 1].hash()) };
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
                expected.entry(parent).or_insert_with(Vec::new).push(header.hash());
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
fn test_retrieve_best_chain() {
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
        assert_eq!(result, chain[4..].iter().map(|h| h.hash()).collect::<Vec<_>>());
    })
}

#[test]
fn load_from_best_chain_root_header() {
    with_db(|store| {
        let chain = populate_db(store.clone());
        let root = run_strategy(any_header_with_parent(chain[0].hash()));

        store.roll_forward_chain(&root.point()).expect("should roll forward successfully");

        assert_eq!(store.load_from_best_chain(&root.point()), Some(root.hash()));
        assert_eq!(store.get_best_chain_hash(), root.hash());
    });
}

#[test]
fn update_best_chain_to_block_slot_given_new_block_is_valid() {
    with_db(|store| {
        let chain = populate_db(store.clone());
        let new_tip = run_strategy(any_header_with_parent(chain[9].hash()));

        store.roll_forward_chain(&new_tip.point()).expect("should roll forward successfully");

        assert_eq!(store.load_from_best_chain(&new_tip.point()), Some(new_tip.hash()));
        assert_eq!(store.get_best_chain_hash(), new_tip.hash());
    });
}

#[test]
fn switch_to_fork_switches_to_fork_and_updates_tip() {
    with_db(|store| {
        let headers = make_forked_headers();
        append_best_chain(store.clone(), headers.main());
        for header in [&headers.h2a, &headers.h3a] {
            store.store_header(header).unwrap();
        }

        store
            .switch_to_fork(&headers.h1.point(), &[headers.h2a.point(), headers.h3a.point()])
            .expect("should replace the best chain successfully");

        assert_eq!(store.get_best_chain_hash(), headers.h3a.hash());
        assert_eq!(store.load_from_best_chain(&headers.h3.point()), None);
        assert_eq!(store.load_from_best_chain(&headers.h2a.point()), Some(headers.h2a.hash()));
        assert_eq!(store.load_from_best_chain(&headers.h3a.point()), Some(headers.h3a.hash()));
    });
}

#[test]
fn switch_to_fork_raises_error_if_fork_point_is_not_on_best_chain() {
    with_db(|store| {
        let headers = make_forked_headers();
        append_best_chain(store.clone(), headers.main());
        for header in [&headers.h2a, &headers.h3a] {
            store.store_header(header).unwrap();
        }

        let result = store.switch_to_fork(&headers.h2a.point(), &[headers.h3a.point()]);

        if result.is_ok() {
            panic!("expected test to fail");
        }
    });
}

#[test]
fn switch_to_fork_preserves_state_when_fork_point_is_not_on_best_chain() {
    // Atomicity: a switch_to_fork call that fails its fork-point check must leave both
    // the chain index AND the best-tip pointer unchanged from pre-call state.
    with_db(|store| {
        let headers = make_forked_headers();
        append_best_chain(store.clone(), headers.main());
        for header in [&headers.h2a, &headers.h3a] {
            store.store_header(header).unwrap();
        }

        let best_chain_before = store.get_best_chain_hash();
        let chain_before = store.retrieve_best_chain();

        let result = store.switch_to_fork(&headers.h2a.point(), &[headers.h3a.point()]);
        assert!(result.is_err(), "expected fork-point-not-on-chain error");

        assert_eq!(store.get_best_chain_hash(), best_chain_before, "best tip must not move");
        assert_eq!(store.retrieve_best_chain(), chain_before, "chain index must be unchanged");
        assert_eq!(store.load_from_best_chain(&headers.h3.point()), Some(headers.h3.hash()));
        assert_eq!(store.load_from_best_chain(&headers.h3a.point()), None);
    });
}

#[test]
fn find_ancestor_on_best_chain_returns_none_when_start_header_is_not_in_store() {
    with_db(|store| {
        let headers = make_forked_headers();
        append_best_chain(store.clone(), headers.main());
        store.set_anchor_hash(&headers.h0.hash()).unwrap();

        let absent = run_strategy(any_header_hash());
        assert_eq!(
            store.find_ancestor_on_best_chain(absent).unwrap(),
            FindAncestorOnBestChainResult::StartHeaderNotFound
        );
    });
}

#[test]
fn find_ancestor_on_best_chain_handles_one_block_fork_off_non_tip() {
    // Best chain: h0 -> h1 -> h2 -> h3
    //                   \
    //                    -> h2a (start, single-block fork off h1)
    with_db(|store| {
        let headers = make_forked_headers();
        append_best_chain(store.clone(), headers.main());
        store.set_anchor_hash(&headers.h0.hash()).unwrap();
        store.store_header(&headers.h2a).unwrap();

        let result = store.find_ancestor_on_best_chain(headers.h2a.hash()).unwrap();
        assert_eq!(
            result,
            FindAncestorOnBestChainResult::Found {
                fork_point: headers.h1.point(),
                forward_points: NonEmptyVec::singleton(headers.h2a.point())
            }
        );
    });
}

#[test]
fn find_missing_blocks_preserves_boundary_parent_invariant_under_truncation() {
    // For any non-empty return, missing[0].parent() == boundary, regardless of limit.
    with_db(|store| {
        // h0 -> ... -> h9, all headers stored, block present only for h0.
        let chain = populate_db(store.clone());
        let block = RawBlock::from(&*vec![1; 64]);
        store.store_block(&chain[0].hash(), &block).unwrap();

        for limit in 1..=9usize {
            let MissingBlocksResult::Found(range) = store.find_missing_blocks(chain[9].hash(), limit).unwrap() else {
                panic!("expected missing blocks");
            };
            let boundary = range.boundary();
            let first_missing = range.first().expect("non-empty missing list with block gap present");
            let first_missing_header = store.load_header(&first_missing.hash()).expect("first missing header exists");
            assert_eq!(
                first_missing_header.parent(),
                Some(boundary.hash()),
                "invariant broken at limit={}: missing[0].parent() != boundary",
                limit,
            );
            assert!(
                range.nb_missing_blocks() <= limit,
                "truncation not respected at limit={}: got {}",
                limit,
                range.nb_missing_blocks(),
            );
        }
    });
}

#[test]
fn next_best_chain_returns_successor_give_valid_point() {
    with_db(|store| {
        let chain = populate_db(store.clone());
        let result = store.next_best_chain(&chain[5].point()).expect("should find successor");
        assert_eq!(result, chain[6].point());
    });
}

#[test]
fn next_best_chain_returns_first_point_on_chain_given_origin() {
    with_db(|store| {
        let chain = populate_db(store.clone());
        let result = store.next_best_chain(&Point::Origin).expect("should find successor");
        assert_eq!(result, chain[0].point());
    });
}

#[test]
fn next_best_chain_returns_slot_zero_point_given_origin() {
    with_db(|store| {
        let h0 = BlockHeader::from(make_header(1, 0, None));
        let h1 = BlockHeader::from(make_header(2, 1, Some(h0.hash())));
        let chain = vec![h0, h1];
        append_best_chain(store.clone(), &chain);

        let result = store.next_best_chain(&Point::Origin).expect("should find successor");

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
fn next_best_chain_header_rolls_forward_from_best_chain_pointer() {
    with_db(|store| {
        let chain = populate_db(store.clone());
        let result = store.next_best_chain_header(&chain[5].point()).unwrap();
        assert_eq!(result, NextBestChainHeader::RollForward { point: chain[6].point(), header: chain[6].clone() });
    });
}

#[test]
fn next_best_chain_header_rolls_forward_from_origin() {
    with_db(|store| {
        let chain = populate_db(store.clone());
        let result = store.next_best_chain_header(&Point::Origin).unwrap();
        assert_eq!(result, NextBestChainHeader::RollForward { point: chain[0].point(), header: chain[0].clone() });
    });
}

#[test]
fn next_best_chain_header_requests_rollback_for_non_best_chain_pointer() {
    with_db(|store| {
        let headers = make_forked_headers();
        append_best_chain(store.clone(), headers.main());
        for header in [&headers.h2a, &headers.h3a] {
            store.store_header(header).unwrap();
        }

        let result = store.next_best_chain_header(&headers.h3a.point()).unwrap();
        assert_eq!(result, NextBestChainHeader::NeedRollback);
    });
}

#[test]
fn next_best_chain_header_reports_at_tip() {
    with_db(|store| {
        let chain = populate_db(store.clone());
        let result = store.next_best_chain_header(&chain[9].point()).unwrap();
        assert_eq!(result, NextBestChainHeader::AtTip);
    });
}

#[test]
fn find_anchor_at_height_returns_first_header_at_or_above_target() {
    with_db(|store| {
        // populate_db sets the anchor to chain[0] (block_height = 1) and best chain to chain[9].
        let chain = populate_db(store.clone());

        let result = store.find_anchor_at_height(BlockHeight::from(5));
        assert_eq!(result, Some(chain[4].hash()));
    });
}

#[test]
fn find_anchor_at_height_returns_none_when_target_at_or_below_current_anchor() {
    with_db(|store| {
        // Anchor is at chain[0] (block_height = 1).
        let _chain = populate_db(store.clone());

        assert!(store.find_anchor_at_height(BlockHeight::from(0)).is_none());
        assert!(store.find_anchor_at_height(BlockHeight::from(1)).is_none());
    });
}

#[test]
fn find_anchor_at_height_returns_none_when_target_beyond_best_chain() {
    with_db(|store| {
        // Best chain tip is chain[9] (block_height = 10).
        let _chain = populate_db(store.clone());

        assert!(store.find_anchor_at_height(BlockHeight::from(100)).is_none());
    });
}

#[test]
fn find_anchor_at_height_walks_from_origin_when_anchor_is_origin() {
    with_db(|store| {
        // Do not set an anchor; leave it at ORIGIN. Only roll forward the best chain.
        let chain = run_strategy(any_headers_chain(10));
        for header in chain.iter() {
            store.store_header(header).unwrap();
            store.roll_forward_chain(&header.point()).unwrap();
        }
        assert_eq!(store.get_anchor_hash(), ORIGIN_HASH);

        let result = store.find_anchor_at_height(BlockHeight::from(3));
        assert_eq!(result, Some(chain[2].hash()));
    });
}

#[test]
fn unvalidated_ancestor_hashes_returns_missing_validity_segment_in_chain_order() {
    with_db(|store| {
        // h0 -> h1(valid) -> h2(?) -> h3(start)
        //        \
        //         -> h2a -> h3a
        let headers = make_forked_headers();
        for header in headers.all() {
            store.store_header(header).unwrap();
        }
        store.set_block_valid(&headers.h1.hash(), true).unwrap();

        let result = store.unvalidated_ancestor_hashes(headers.h3.hash());
        assert_eq!(result, (vec![headers.h2.hash(), headers.h3.hash()], true));
    });
}

#[test]
fn unvalidated_ancestor_hashes_marks_chain_invalid_when_it_hits_invalid_ancestor() {
    with_db(|store| {
        // h0 -> h1(invalid) -> h2(?) -> h3(start)
        //          \
        //           -> h2a -> h3a
        let headers = make_forked_headers();
        for header in headers.all() {
            store.store_header(header).unwrap();
        }
        store.set_block_valid(&headers.h1.hash(), false).unwrap();

        let result = store.unvalidated_ancestor_hashes(headers.h3.hash());
        assert_eq!(result, (vec![headers.h2.hash(), headers.h3.hash()], false));
    });
}

#[test]
fn find_ancestor_on_best_chain_returns_best_chain_intersection_and_forward_path() {
    with_db(|store| {
        // Best chain: h0 -> h1 -> h2 -> h3
        //                   \
        // fork:              -> h2a -> h3a(start)
        let headers = make_forked_headers();
        append_best_chain(store.clone(), headers.main());
        store.set_anchor_hash(&headers.h0.hash()).unwrap();
        for header in [&headers.h2a, &headers.h3a] {
            store.store_header(header).unwrap();
        }

        let result = store.find_ancestor_on_best_chain(headers.h3a.hash()).unwrap();
        assert_eq!(
            result,
            FindAncestorOnBestChainResult::Found {
                fork_point: headers.h1.point(),
                forward_points: NonEmptyVec::new(headers.h2a.point(), vec![headers.h3a.point()])
            }
        );
    });
}

#[test]
fn find_ancestor_on_best_chain_returns_none_when_start_is_already_on_best_chain() {
    with_db(|store| {
        let headers = make_forked_headers();
        append_best_chain(store.clone(), headers.main());
        store.set_anchor_hash(&headers.h0.hash()).unwrap();

        let result = store.find_ancestor_on_best_chain(headers.h3.hash()).unwrap();
        assert_eq!(result, FindAncestorOnBestChainResult::NotFound);
    });
}

#[test]
fn find_common_ancestor_returns_shared_point_between_forks() {
    with_db(|store| {
        // h0 -> h1 -> h2 -> h3
        //        \
        //         -> h2a -> h3a
        // common_ancestor(h3, h3a) = h1
        let headers = make_forked_headers();
        for header in headers.all() {
            store.store_header(header).unwrap();
        }

        let result = store.find_common_ancestor(headers.h3.hash(), headers.h3a.hash()).unwrap();
        assert_eq!(result, FindCommonAncestorResult::Found(headers.h1.point()));
    });
}

#[test]
fn sample_ancestor_points_returns_exponential_walk_back_from_best_tip() {
    with_db(|store| {
        // Best chain:
        // h0 -> h1 -> h2 -> h3 -> h4 -> h5 -> h6 -> h7 -> h8 -> h9 (tip)
        // Samples:
        // h9, h8, h7, h5, h1, h0
        let chain = populate_db(store.clone());

        let result = store.sample_ancestor_points().unwrap();

        assert_eq!(
            result,
            SampleAncestorPointsResult::Found(vec![
                chain[9].point(),
                chain[8].point(),
                chain[7].point(),
                chain[5].point(),
                chain[1].point(),
                chain[0].point(),
            ])
        );
    });
}

#[test]
fn test_intersect_points_includes_best_point_and_are_spaced_with_a_factor_2() {
    with_db(|store| {
        let mut parent = None;
        for slot in 0..=100 {
            let header = BlockHeader::from(make_header(slot + 1, slot, parent));
            store.store_header(&header).unwrap();
            store.roll_forward_chain(&header.point()).unwrap();
            parent = Some(header.hash());
        }

        let SampleAncestorPointsResult::Found(result) = store.sample_ancestor_points().unwrap() else {
            panic!("no ancestor point could be sampled");
        };
        let slots = result.iter().map(|point| u64::from(point.slot_or_default())).collect::<Vec<_>>();

        assert_eq!(slots, vec![100, 99, 98, 96, 92, 84, 68, 36, 0]);
    });
}

#[test]
fn find_missing_blocks_returns_path_from_nearest_available_block_to_tip() {
    with_db(|store| {
        // Best chain:
        // h0 -> h1 -> h2 -> h3 -> h4 -> h5 -> h6 -> h7 -> h8 -> h9 (tip)
        //                                     *
        //                                 block present
        // Missing path to fetch from h9:
        // h6 -> h7 -> h8 -> h9
        let chain = populate_db(store.clone());
        let block = RawBlock::from(&*vec![1; 64]);
        store.store_block(&chain[6].hash(), &block).unwrap();

        let result = store.find_missing_blocks(chain[9].hash(), 10).unwrap();

        assert_eq!(
            result,
            MissingBlocksResult::Found(MissingBlocks::new(
                chain[6].point(),
                vec![chain[7].point(), chain[8].point(), chain[9].point()],
            ))
        );
    });
}

#[test]
fn find_missing_blocks_returns_none_when_tip_is_not_found() {
    with_db(|store| {
        let missing_tip = run_strategy(any_header_hash());
        let result = store.find_missing_blocks(missing_tip, 10).unwrap();
        assert_eq!(result, MissingBlocksResult::StartHeaderNotFound);
    });
}

#[test]
fn find_missing_blocks_returns_boundary_only_when_tip_block_exists() {
    with_db(|store| {
        // Best chain:
        // h0 -> h1 -> h2 -> h3 -> h4 -> h5 -> h6 -> h7 -> h8 -> h9 (tip)
        //                                                 *
        //                                              block present
        let chain = populate_db(store.clone());
        let block = RawBlock::from(&*vec![1; 64]);
        store.store_block(&chain[9].hash(), &block).unwrap();

        let result = store.find_missing_blocks(chain[9].hash(), 10).unwrap();
        assert_eq!(result, MissingBlocksResult::Found(MissingBlocks::new(chain[9].point(), vec![])));
    });
}

#[test]
fn read_snapshot_keeps_original_best_chain_view_after_store_changes() {
    with_read_db(
        |store| {
            populate_db(store);
        },
        |store, snapshot| {
            let best_chain_hash = snapshot.get_best_chain_hash();
            let tip = snapshot.load_header(&best_chain_hash).expect("tip should exist in snapshot");
            let next_slot = u64::from(tip.slot()) + 1;
            let new_tip = BlockHeader::from(make_header(next_slot, next_slot, Some(tip.hash())));

            store.store_header(&new_tip).expect("should store header successfully");
            store.roll_forward_chain(&new_tip.point()).expect("should roll forward successfully");
            assert_eq!(snapshot.get_best_chain_hash(), best_chain_hash);
            assert_eq!(snapshot.load_from_best_chain(&new_tip.point()), None);
            assert!(snapshot.next_best_chain(&tip.point()).is_none());
        },
    );
}

#[test]
fn read_snapshot_exposes_direct_read_operations() {
    let headers = make_forked_headers();
    let nonces = Nonces {
        active: Nonce::from(random_bytes(32).as_slice()),
        evolving: Nonce::from(random_bytes(32).as_slice()),
        candidate: Nonce::from(random_bytes(32).as_slice()),
        tail: headers.h1.hash(),
        epoch: Default::default(),
    };
    let block = RawBlock::from(&*vec![1; 64]);

    with_read_db(
        {
            let headers = headers.clone();
            let nonces = nonces.clone();
            let block = block.clone();
            move |store| {
                append_best_chain(store.clone(), headers.main());
                for header in [&headers.h2a, &headers.h3a] {
                    store.store_header(header).unwrap();
                }
                store.set_anchor_hash(&headers.h0.hash()).unwrap();
                store.put_nonces(&headers.h2.hash(), &nonces).unwrap();
                store.store_block(&headers.h3.hash(), &block).unwrap();
                store.set_block_valid(&headers.h2.hash(), true).unwrap();
            }
        },
        {
            let headers = headers.clone();
            let nonces = nonces.clone();
            let block = block.clone();
            move |_store, snapshot| {
                let mut children = snapshot.get_children(&headers.h1.hash());
                children.sort();

                assert_eq!(snapshot.load_header(&headers.h2.hash()), Some(headers.h2.clone()));
                assert_eq!(
                    snapshot.load_header_with_validity(&headers.h2.hash()),
                    Some((headers.h2.clone(), Some(true)))
                );
                assert_eq!(children, vec![headers.h2.hash(), headers.h2a.hash()]);
                assert_eq!(snapshot.get_anchor_hash(), headers.h0.hash());
                assert_eq!(snapshot.get_best_chain_hash(), headers.h3.hash());
                assert_eq!(snapshot.get_nonces(&headers.h2.hash()), Some(nonces.clone()));
                assert_eq!(snapshot.load_block(&headers.h3.hash()).unwrap(), Some(block.clone()));
                assert!(snapshot.has_header(&headers.h3a.hash()));
                assert!(!snapshot.has_header(&run_strategy(any_header_hash())));
            }
        },
    );
}

#[test]
fn read_snapshot_supports_best_chain_traversal() {
    let chain = make_linear_headers(10);

    with_read_db(
        {
            let chain = chain.clone();
            move |store| {
                store.set_anchor_hash(&chain[0].hash()).unwrap();
                append_best_chain(store.clone(), &chain);
            }
        },
        {
            let chain = chain.clone();
            move |store, snapshot| {
                let invalid_point = Point::Specific(100.into(), run_strategy(any_header_hash()));

                assert_eq!(store.retrieve_best_chain(), chain.iter().map(BlockHeader::hash).collect::<Vec<_>>());
                assert_eq!(snapshot.load_from_best_chain(&chain[0].point()), Some(chain[0].hash()));
                assert_eq!(snapshot.load_from_best_chain(&invalid_point), None);
                assert_eq!(snapshot.next_best_chain(&Point::Origin), Some(chain[0].point()));
                assert_eq!(snapshot.next_best_chain(&chain[5].point()), Some(chain[6].point()));
                assert_eq!(snapshot.next_best_chain(&chain[9].point()), None);
            }
        },
    );
}

#[test]
fn read_snapshot_supports_best_chain_traversal_from_origin_to_slot_zero() {
    let h0 = BlockHeader::from(make_header(1, 0, None));
    let h1 = BlockHeader::from(make_header(2, 1, Some(h0.hash())));
    let chain = vec![h0, h1];

    with_read_db(
        {
            let chain = chain.clone();
            move |store| {
                append_best_chain(store.clone(), &chain);
            }
        },
        {
            let chain = chain.clone();
            move |_store, snapshot| {
                assert_eq!(snapshot.next_best_chain(&Point::Origin), Some(chain[0].point()));
                assert_eq!(snapshot.next_best_chain(&chain[0].point()), Some(chain[1].point()));
            }
        },
    );
}

#[test]
fn read_snapshot_supports_find_intersect_point_queries() {
    let chain = make_linear_headers(10);

    with_read_db(
        {
            let chain = chain.clone();
            move |store| {
                append_best_chain(store.clone(), &chain);
                store.set_anchor_hash(&chain[5].hash()).unwrap();
            }
        },
        {
            let chain = chain.clone();
            move |store, _snapshot| {
                let unknown = Point::Specific(Slot::from(999), Hash::new([0xff; HEADER]));

                // intersect one point
                assert_eq!(store.find_intersect_point(vec![chain[5].point()]), Some(chain[5].point()));
                // intersect the most recent point
                assert_eq!(
                    store.find_intersect_point(vec![chain[3].point(), chain[7].point()]),
                    Some(chain[7].point())
                );
                // intersect the most recent point
                assert_eq!(store.find_intersect_point(vec![chain[2].point()]), Some(chain[2].point()));
                // intersect with no points
                assert_eq!(store.find_intersect_point(vec![]), None);
                // intersect with an unknown pointThe thing is not comparable.
                assert_eq!(store.find_intersect_point(vec![unknown]), None);
            }
        },
    );
}

#[test]
fn find_intersect_point_returns_origin_when_best_chain_is_non_empty() {
    with_db(|store| {
        let _chain = populate_db(store.clone());

        assert_eq!(store.find_intersect_point(vec![Point::Origin]), Some(Point::Origin));
    });
}

#[test]
fn read_snapshot_supports_ancestor_fork_and_sampling_queries() {
    let headers = make_forked_headers();
    let chain = make_linear_headers(10);

    with_read_db(
        {
            let headers = headers.clone();
            let chain = chain.clone();
            move |store| {
                append_best_chain(store.clone(), headers.main());
                for header in [&headers.h2a, &headers.h3a] {
                    store.store_header(header).unwrap();
                }
                store.set_anchor_hash(&headers.h0.hash()).unwrap();
                store.set_block_valid(&headers.h1.hash(), true).unwrap();

                append_best_chain(store.clone(), &chain[4..]);
            }
        },
        {
            let headers = headers.clone();
            let chain = chain.clone();
            move |store, _snapshot| {
                assert_eq!(
                    store.unvalidated_ancestor_hashes(headers.h3.hash()),
                    (vec![headers.h2.hash(), headers.h3.hash()], true)
                );
                assert_eq!(
                    store.find_ancestor_on_best_chain(headers.h3a.hash()).unwrap(),
                    FindAncestorOnBestChainResult::Found {
                        fork_point: headers.h1.point(),
                        forward_points: NonEmptyVec::new(headers.h2a.point(), vec![headers.h3a.point()])
                    }
                );
                assert_eq!(
                    store.find_common_ancestor(headers.h3.hash(), headers.h3a.hash()).unwrap(),
                    FindCommonAncestorResult::Found(headers.h1.point())
                );
                assert_eq!(
                    store.sample_ancestor_points().unwrap(),
                    SampleAncestorPointsResult::Found(vec![
                        chain[9].point(),
                        chain[8].point(),
                        chain[7].point(),
                        chain[5].point(),
                        chain[1].point(),
                        chain[0].point(),
                    ])
                );
            }
        },
    );
}

#[test]
fn read_snapshot_supports_missing_block_queries() {
    let chain = make_linear_headers(10);
    let block = RawBlock::from(&*vec![1; 64]);

    with_read_db(
        {
            let chain = chain.clone();
            let block = block.clone();
            move |store| {
                store.set_anchor_hash(&chain[0].hash()).unwrap();
                append_best_chain(store.clone(), &chain);
                store.store_block(&chain[6].hash(), &block).unwrap();
            }
        },
        {
            let chain = chain.clone();
            move |store, _snapshot| {
                let missing_tip = run_strategy(any_header_hash());

                assert_eq!(
                    store.find_missing_blocks(chain[9].hash(), 10).unwrap(),
                    MissingBlocksResult::Found(MissingBlocks::new(
                        chain[6].point(),
                        vec![chain[7].point(), chain[8].point(), chain[9].point()],
                    ))
                );
                assert_eq!(
                    store.find_missing_blocks(missing_tip, 10).unwrap(),
                    MissingBlocksResult::StartHeaderNotFound
                );
            }
        },
    );
}

#[test]
fn read_snapshot_supports_child_tips_all() {
    let headers = make_forked_headers();
    let block = RawBlock::from(&*vec![1; 64]);

    with_read_db(
        {
            let block = block.clone();
            let headers = headers.clone();
            move |store| {
                for h in headers.all() {
                    store.store_header(h).unwrap();
                    store.store_block(&h.hash(), &block).unwrap();
                }
            }
        },
        {
            move |store, _snapshot| {
                assert_eq!(
                    store.child_tips(&headers.h0.hash(), ChildTipsMode::All),
                    vec![headers.h3.tip(), headers.h3a.tip()],
                    "\nheaders\n{headers}"
                );
            }
        },
    );
}

#[test]
fn read_snapshot_supports_child_tips_skip_invalid() {
    let headers = make_forked_headers();
    let block = RawBlock::from(&*vec![1; 64]);

    with_read_db(
        {
            let block = block.clone();
            let headers = headers.clone();
            move |store| {
                for h in headers.all() {
                    store.store_header(h).unwrap();
                    store.store_block(&h.hash(), &block).unwrap();
                    store.set_block_valid(&h.hash(), h.hash() != headers.h2.hash()).unwrap();
                }
            }
        },
        {
            move |store, _snapshot| {
                assert_eq!(
                    store.child_tips(&headers.h0.hash(), ChildTipsMode::SkipInvalid),
                    vec![headers.h3a.tip()],
                    "\nheaders\n{headers}"
                );
            }
        },
    );
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

    let store = RocksDBStore::create(RocksDbConfig::new(basedir)).expect("should create DB successfully");
    let version = get_version(&store).expect("should read version successfully");

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

    let (basedir, db) = open_db(&config).expect("cannot open sample v0 DB");
    let store = RocksDBStore { db, basedir };
    migrate_to_v1(&store).expect("Migration should succeed");

    let header: Option<BlockHeader> = store
        .db
        .get_pinned([&HEADER_PREFIX[..], hex::decode(SAMPLE_HASH).unwrap().as_slice()].concat())
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

    let db = RocksDBStore::open(&config).expect("DB should successfully be opened as it's been migrated");
    assert_eq!((1, 3), result);
    let header: Option<HeaderHash> = <RocksDBStore as BaseReadChainStore>::load_from_best_chain(
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
    let version = get_version(&store).expect("should read version successfully");

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
        db.put(&prefix, header_hash).expect("should put data successfully");
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
    db.delete([&CHAIN_PREFIX[..], &slot6].concat()).expect("should delete data successfully");
    db.delete([&CHAIN_PREFIX[..], &slot7].concat()).expect("should delete data successfully");

    // iterator continues from where it left off, skipping deleted keys
    assert_eq!(*(iter.next().unwrap().unwrap().0), [&CHAIN_PREFIX[..], &slot8].concat());
    assert_eq!(*(iter.next().unwrap().unwrap().0), [&CHAIN_PREFIX[..], &slot9].concat());
}

// HELPERS

const SAMPLE_HASH: &str = "4b1f95026700f5b3df8432b3f93b023f3cbdf13c85704e0f71b0089e6e81c947";

#[derive(Clone)]
struct ForkedHeaders {
    h0: BlockHeader,
    h1: BlockHeader,
    h2: BlockHeader,
    h3: BlockHeader,
    h2a: BlockHeader,
    h3a: BlockHeader,
}

impl Display for ForkedHeaders {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "h0 {}.{:.6}", self.h0.slot(), self.h0.hash().to_string())?;
        write!(f, " -> h1: {}.{:.6}", self.h1.slot(), self.h1.hash().to_string())?;
        write!(f, " -> h2:  {}.{:.6}", self.h2.slot(), self.h2.hash().to_string())?;
        writeln!(f, "  -> h3:  {}.{:.6}", self.h3.slot(), self.h3.hash().to_string())?;
        write!(f, "                            -> h2a: {}.{:.6}", self.h2a.slot(), self.h2a.hash().to_string())?;
        write!(f, " -> h3a: {}.{:.6}", self.h3a.slot(), self.h3a.hash().to_string())?;
        Ok(())
    }
}

impl ForkedHeaders {
    fn main(&self) -> [&BlockHeader; 4] {
        [&self.h0, &self.h1, &self.h2, &self.h3]
    }

    fn all(&self) -> [&BlockHeader; 6] {
        [&self.h0, &self.h1, &self.h2, &self.h3, &self.h2a, &self.h3a]
    }
}

/// h0 -> h1 -> h2 -> h3
///          -> h2a -> h3a
///
fn make_forked_headers() -> ForkedHeaders {
    let h0 = BlockHeader::from(make_header(1, 1, None));
    let h1 = BlockHeader::from(make_header(2, 2, Some(h0.hash())));
    let h2 = BlockHeader::from(make_header(3, 3, Some(h1.hash())));
    let h3 = BlockHeader::from(make_header(4, 4, Some(h2.hash())));
    let h2a = BlockHeader::from(make_header(3, 10, Some(h1.hash())));
    let h3a = BlockHeader::from(make_header(4, 11, Some(h2a.hash())));

    ForkedHeaders { h0, h1, h2, h3, h2a, h3a }
}

fn make_linear_headers(len: usize) -> Vec<BlockHeader> {
    let mut headers = Vec::with_capacity(len);
    for i in 0..len {
        let parent = headers.last().map(BlockHeader::hash);
        headers.push(BlockHeader::from(make_header((i + 1) as u64, (i + 1) as u64, parent)));
    }
    headers
}

fn append_best_chain<'a>(store: Arc<dyn ChainStore>, headers: impl IntoIterator<Item = &'a BlockHeader>) {
    for header in headers {
        store.store_header(header).unwrap();
        store.roll_forward_chain(&header.point()).unwrap();
    }
}

fn populate_db(store: Arc<dyn ChainStore>) -> Vec<BlockHeader> {
    let chain = run_strategy(any_headers_chain(10));

    // Set the anchor to the first header in the chain
    store.set_anchor_hash(&chain[0].hash()).expect("should set anchor hash successfully");

    for header in chain.iter() {
        store.roll_forward_chain(&header.point()).expect("should roll forward successfully");
        store.store_header(header).expect("should store header successfully");
    }
    chain
}

pub fn initialise_test_rw_store(path: &Path) -> RocksDBStore {
    let basedir = init_dir(path);
    let config = RocksDbConfig::new(basedir);

    RocksDBStore::create(config).expect("fail to initialise RocksDB")
}

pub fn initialise_test_ro_store(path: &Path) -> Result<RocksDBStore<DB>, StoreError> {
    let basedir = init_dir(path);
    RocksDBStore::open_for_readonly(&RocksDbConfig::new(basedir))
}

fn init_dir(path: &Path) -> PathBuf {
    let basedir = path.join("rocksdb_chain_store");
    use std::fs::create_dir_all;
    create_dir_all(&basedir).expect("fail to create test dir");
    basedir
}

fn with_db(f: impl Fn(Arc<dyn FullChainStore>)) {
    // try first with in-memory store
    let in_memory_store: Arc<dyn FullChainStore> = Arc::new(InMemoryChainStore::new());
    f(in_memory_store);

    // then with rocksdb store
    let tempdir = tempfile::tempdir().unwrap();
    let rw_store: Arc<dyn FullChainStore> = Arc::new(initialise_test_rw_store(tempdir.path()));
    f(rw_store);
}

fn with_read_db(
    setup: impl Fn(Arc<dyn ChainStore>),
    assert: impl Fn(Arc<dyn FullChainStore>, &dyn BaseReadChainStore),
) {
    let in_memory_store: Arc<dyn FullChainStore> = Arc::new(InMemoryChainStore::new());
    // Initialize the store and take a snapshot
    setup(in_memory_store.clone());
    let snapshot = in_memory_store.snapshot();
    // check assertions against the in-memory snapshot
    assert(in_memory_store.clone(), snapshot.as_ref());

    let tempdir = tempfile::tempdir().unwrap();
    let rw_store: Arc<dyn FullChainStore> = Arc::new(initialise_test_rw_store(tempdir.path()));
    // Initialize the store and take a snapshot
    setup(rw_store.clone());
    let snapshot = rw_store.snapshot();
    // check assertions against the RocksDB snapshot
    assert(rw_store.clone(), snapshot.as_ref());
}

fn with_db_path(f: impl Fn((Arc<dyn ChainStore>, &Path))) {
    let tempdir = tempfile::tempdir().unwrap();
    let rw_store: Arc<dyn ChainStore> = Arc::new(initialise_test_rw_store(tempdir.path()));
    f((rw_store, tempdir.path()));
}

fn sort_entries(mut v: Vec<(HeaderHash, Vec<HeaderHash>)>) -> Vec<(HeaderHash, Vec<HeaderHash>)> {
    v.sort_by_key(|(k, _)| *k);
    for (_, children) in &mut v {
        children.sort();
    }
    v
}

// from https://nick.groenen.me/notes/recursively-copy-files-in-rust/
// NOTE: the stored database is only valid for Unix (Linux/MacOS) systems, so
// any test relying on it should be guarded for not running on windows
pub fn copy_recursively(source: impl AsRef<Path>, destination: impl AsRef<Path>) -> io::Result<()> {
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
