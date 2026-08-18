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

use std::collections::BTreeMap;

pub use amaru_ledger::store::{
    StoreError,
    columns::{
        proposals::{Key, Row, Value},
        unsafe_decode,
    },
};
use amaru_observability::trace_span;
use rocksdb::{DBPinnableSlice, Transaction};

use crate::rocksdb::{
    common::{PREFIX_LEN, as_key, as_value},
    votes,
};

/// Name prefixed used for storing Proposals entries. UTF-8 encoding for "prop"
pub const PREFIX: [u8; PREFIX_LEN] = *b"prop";

/// Retrieve a single governance proposal.
pub fn get<'a>(
    db_get: impl Fn(&[u8]) -> Result<Option<DBPinnableSlice<'a>>, rocksdb::Error>,
    id: &Key,
) -> Result<Option<Row>, StoreError> {
    trace_span!(stores::ledger::proposals::GET).in_scope(|| {
        let key = as_key(&PREFIX, id);
        let bytes = db_get(&key);
        bytes.map_err(|err| StoreError::Internal(err.into())).map(|opt| opt.map(|d| unsafe_decode::<Row>(&d)))
    })
}

/// Register a new Proposal.
pub fn add<DB>(db: &Transaction<'_, DB>, rows: impl Iterator<Item = (Key, Value)>) -> Result<usize, StoreError> {
    trace_span!(stores::ledger::proposals::ADD).in_scope(|| {
        let mut n = 0;

        for (key, value) in rows {
            n += 1;
            db.put(as_key(&PREFIX, key), as_value(value)).map_err(|err| StoreError::Internal(err.into()))?;
        }

        Ok(n)
    })
}

/// Remove an expired or enacted proposal and the votes that pertains to it.
pub fn remove<DB, V>(db: &Transaction<'_, DB>, proposals: &BTreeMap<Key, V>) -> Result<(), StoreError> {
    trace_span!(stores::ledger::proposals::REMOVE).in_scope(|| {
        for key in proposals.keys() {
            votes::iter_by_proposal(db, key)?.try_for_each(|vote| votes::remove(db, vote))?;
            db.delete(as_key(&PREFIX, key)).map_err(|err| StoreError::Internal(err.into()))?;
        }

        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::LazyLock,
    };

    use amaru_kernel::{Ballot, BallotId, ProposalId, Voter, any_ballot, any_proposal, any_proposal_id, any_voter};
    use amaru_ledger::store::{ReadStore, Store};
    use proptest::{
        collection::{btree_map, btree_set},
        prelude::*,
    };
    use tempfile::TempDir;

    use crate::rocksdb::{RocksDB, RocksDbConfig, StoreError, proposals, votes};

    // Re-using the database connection for property tests considerably speed things up. One must
    // `.clear()` the database at the start of each test, though.
    static DB: LazyLock<RocksDB> = LazyLock::new(|| {
        let tmp = TempDir::new().expect("failed to create temp dir");
        RocksDB::empty(&RocksDbConfig::new(tmp.path().into())).unwrap()
    });

    prop_compose! {
        fn any_proposal_row()(proposal in any_proposal()) -> proposals::Row {
            proposals::Row {
                proposed_in: Default::default(),
                valid_until: Default::default(),
                proposal,
            }
        }
    }

    proptest! {
        #[test]
        fn remove(
            proposals in btree_map(any_proposal_id(), any_proposal_row(), 2..=3),
            votes in btree_map(any_voter(), any_ballot(), 100),
            indices_to_remove in btree_set(any::<usize>(), 1..=3),
        ) {
            DB.clear().unwrap();

            prop_assume!(DB.iter_votes()?.collect::<Vec<_>>().is_empty());

            let indices_to_remove: BTreeSet<usize> = indices_to_remove.into_iter().map(|ix| ix % proposals.len()).collect();

            let mut votes_count_before = fixture_proposals_and_votes(&DB, proposals.clone().into_iter(), votes.into_iter());

            let (removed, kept): (BTreeMap<ProposalId, ()>, BTreeSet<ProposalId>) = proposals.keys().enumerate().fold(
                Default::default(),
                |(mut removed, mut kept), (ix, proposal_id)| {
                    if indices_to_remove.contains(&ix) {
                        removed.insert(*proposal_id, ());
                    } else {
                        kept.insert(*proposal_id);
                    }

                    (removed, kept)
                }
            );

            prop_assume!(!removed.is_empty());

            // Remove proposals and associated votes
            DB.with_transaction(|ctx| proposals::remove(&ctx.db, &removed)).unwrap();

            let mut votes_count_after = BTreeMap::new();

            // No votes to that proposal should remain
            for (ballot_id, _) in DB.iter_votes()? {
                votes_count_after.entry(ballot_id.proposal).and_modify(|n| *n += 1).or_insert(1);
                prop_assert!(kept.contains(&ballot_id.proposal), "{ballot_id:?} => {kept:?}");
                prop_assert!(!removed.contains_key(&ballot_id.proposal), "{ballot_id:?} => {removed:?}");
            }

            // Ensures that no other votes are deleted;
            votes_count_before.retain(|k, _| !removed.contains_key(k));
            prop_assert_eq!(votes_count_before, votes_count_after);

            // Proposals should have been pruned or left untouched.
            DB.with_transaction(|ctx| {
                for (proposal_id, proposal) in proposals {
                    prop_assert_eq!(
                        proposals::get(|key| ctx.db.get_pinned(key), &proposal_id).unwrap(),
                        if removed.contains_key(&proposal_id) {
                            None
                        } else {
                            Some(proposal)
                        }
                    );
                }

                Ok(())
            }).unwrap();
        }
    }

    // Prepare the store with some votes, returning a map of the number of votes for each proposal.
    fn fixture_proposals_and_votes(
        db: &RocksDB,
        proposals: impl Iterator<Item = (ProposalId, proposals::Row)>,
        votes: impl Iterator<Item = (Voter, Ballot)>,
    ) -> BTreeMap<ProposalId, usize> {
        let mut count = BTreeMap::new();

        db.with_transaction(|ctx| {
            let proposals = proposals.collect::<Vec<_>>();

            votes::add(
                &ctx.db,
                votes.enumerate().map(|(ix, (voter, ballot))| {
                    let proposal = proposals[ix % proposals.len()].0;
                    count.entry(proposal).and_modify(|n| *n += 1).or_insert(1);
                    let ballot_id = BallotId { voter, proposal };
                    (ballot_id, ballot)
                }),
            )?;

            proposals::add(&ctx.db, proposals.into_iter())?;

            Ok::<_, StoreError>(())
        })
        .unwrap();

        count
    }
}
