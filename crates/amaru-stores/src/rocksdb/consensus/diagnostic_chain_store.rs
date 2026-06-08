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

use amaru_kernel::{BlockHeader, Hash, HeaderHash, RawBlock, from_cbor, size::HEADER};
use amaru_ouroboros_traits::{DiagnosticChainStore, Nonces};
use rocksdb::{DB, IteratorMode, PrefixRange, ReadOptions};

use crate::rocksdb::consensus::{
    RocksDBStore,
    util::{BLOCK_PREFIX, CHILD_PREFIX, CONSENSUS_PREFIX_LEN, HEADER_PREFIX, NONCES_PREFIX},
};

impl DiagnosticChainStore for RocksDBStore {
    #[allow(clippy::panic)]
    fn load_headers(&self) -> Box<dyn Iterator<Item = BlockHeader> + '_> {
        Box::new(self.db.prefix_iterator(HEADER_PREFIX).filter_map(|item| match item {
            Ok((_k, v)) => from_cbor(v.as_ref()),
            Err(err) => panic!("error iterating over headers: {}", err),
        }))
    }

    #[allow(clippy::panic)]
    fn load_nonces(&self) -> Box<dyn Iterator<Item = (HeaderHash, Nonces)> + '_> {
        Box::new(self.db.prefix_iterator(NONCES_PREFIX).filter_map(|item| match item {
            Ok((k, v)) => {
                let hash = Hash::from(&k[CONSENSUS_PREFIX_LEN..]);
                from_cbor(&v).map(|nonces| (hash, nonces))
            }
            Err(err) => panic!("error iterating over nonces: {}", err),
        }))
    }

    #[allow(clippy::panic)]
    fn load_blocks(&self) -> Box<dyn Iterator<Item = (HeaderHash, RawBlock)> + '_> {
        let mut opts = ReadOptions::default();
        opts.set_iterate_range(PrefixRange(&BLOCK_PREFIX[..]));
        Box::new(self.db.iterator_opt(IteratorMode::Start, opts).map(|item| match item {
            Ok((k, v)) => {
                let hash = Hash::from(&k[CONSENSUS_PREFIX_LEN..]);
                (hash, RawBlock::from(v))
            }
            Err(err) => panic!("error iterating over blocks: {}", err),
        }))
    }

    #[allow(clippy::expect_used)]
    fn load_parents_children(&self) -> Box<dyn Iterator<Item = (HeaderHash, Vec<HeaderHash>)> + '_> {
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

impl DiagnosticChainStore for RocksDBStore<DB> {
    #[allow(clippy::panic)]
    fn load_headers(&self) -> Box<dyn Iterator<Item = BlockHeader> + '_> {
        Box::new(self.db.prefix_iterator(HEADER_PREFIX).filter_map(|item| match item {
            Ok((_k, v)) => from_cbor(v.as_ref()),
            Err(err) => panic!("error iterating over headers: {}", err),
        }))
    }

    #[allow(clippy::panic)]
    fn load_nonces(&self) -> Box<dyn Iterator<Item = (HeaderHash, Nonces)> + '_> {
        Box::new(self.db.prefix_iterator(NONCES_PREFIX).filter_map(|item| match item {
            Ok((k, v)) => {
                let hash = Hash::from(&k[CONSENSUS_PREFIX_LEN..]);
                from_cbor(&v).map(|nonces| (hash, nonces))
            }
            Err(err) => panic!("error iterating over nonces: {}", err),
        }))
    }

    #[allow(clippy::panic)]
    fn load_blocks(&self) -> Box<dyn Iterator<Item = (HeaderHash, RawBlock)> + '_> {
        let mut opts = ReadOptions::default();
        opts.set_iterate_range(PrefixRange(&BLOCK_PREFIX[..]));
        Box::new(self.db.iterator_opt(IteratorMode::Start, opts).map(|item| match item {
            Ok((k, v)) => {
                let hash = Hash::from(&k[CONSENSUS_PREFIX_LEN..]);
                (hash, RawBlock::from(v))
            }
            Err(err) => panic!("error iterating over blocks: {}", err),
        }))
    }

    #[allow(clippy::expect_used)]
    fn load_parents_children(&self) -> Box<dyn Iterator<Item = (HeaderHash, Vec<HeaderHash>)> + '_> {
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
