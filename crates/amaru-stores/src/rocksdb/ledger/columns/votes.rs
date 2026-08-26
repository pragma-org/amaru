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

use std::collections::BTreeSet;

use amaru_kernel::{BallotId, Credential, ProposalId, Voter, cbor};
pub use amaru_ledger::store::{
    StoreError,
    columns::votes::{Key, Row, Value},
};
use amaru_observability::trace_span;
use rocksdb::Transaction;

use crate::rocksdb::{
    common::{PREFIX_LEN, as_key, as_value},
    from_store, iter_raw, prefix_successor,
};

/// Name prefixed used for storing Proposals entries. UTF-8 encoding for "vote"
pub const PREFIX: [u8; PREFIX_LEN] = [0x76, 0x6f, 0x74, 0x65];

/// Register a series of new votes. Returns the credentials (script or key) of all dreps found
/// amongst the voters.
pub fn add<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = (Key, Value)>,
) -> Result<BTreeSet<Credential>, StoreError> {
    trace_span!(stores::ledger::votes::ADD).in_scope(|| {
        let mut voting_dreps = BTreeSet::new();

        for (key, value) in rows {
            match key.voter {
                Voter::DRepKey(hash) => {
                    voting_dreps.insert(Credential::KeyHash(hash));
                }
                Voter::DRepScript(hash) => {
                    voting_dreps.insert(Credential::ScriptHash(hash));
                }
                Voter::ConstitutionalCommitteeKey(..)
                | Voter::ConstitutionalCommitteeScript(..)
                | Voter::StakePoolKey(..) => {}
            }

            db.put(as_key(&PREFIX, &key), as_value(value)).map_err(|err| StoreError::Internal(err.into()))?;
        }

        Ok(voting_dreps)
    })
}

#[expect(clippy::expect_used)]
pub fn iter_by_proposal<DB>(
    db: &Transaction<'_, DB>,
    proposal: &ProposalId,
) -> Result<impl Iterator<Item = Key>, StoreError> {
    let mut prefix = Vec::new();
    prefix.extend_from_slice(&PREFIX);
    BallotId::encode_prefix(proposal, &mut cbor::encode::Encoder::new(&mut prefix))
        .unwrap_or_else(|_| unreachable!("encoding to a mutable Vec cannot fail"));

    let lo = prefix.clone();
    let hi = prefix_successor(&prefix[..]).expect("successor always exists here");

    iter_raw(
        |mode, mut opts| {
            // NOTE: RocksDB iterator and prefixes
            //
            // We configure a prefix size of PREFIX_LEN at start; which means that unless we provide
            // an explicit range here; RocksDB will match only based on the first PREFIX_LEN bytes
            // of our prefix and as a consequence, will yield entry we precisely don't want to
            // match!
            //
            // By setting an explicit prefix range, we avoid this headache.
            opts.set_iterate_range(lo..hi);
            db.iterator_opt(mode, opts)
        },
        prefix,
        |key, _| from_store(&key[PREFIX_LEN..]),
    )
}

pub fn remove<DB>(db: &Transaction<'_, DB>, key: Key) -> Result<(), StoreError> {
    trace_span!(stores::ledger::votes::REMOVE)
        .in_scope(|| db.delete(as_key(&PREFIX, &key)).map_err(|err| StoreError::Internal(err.into())))
}
