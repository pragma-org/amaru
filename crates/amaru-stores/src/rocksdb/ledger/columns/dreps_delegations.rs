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

use crate::rocksdb::{PREFIX_LEN, as_value, dreps};
use ::rocksdb::{Direction, IteratorMode, ReadOptions, Transaction};
use amaru_kernel::{
    CertificatePointer, DRep, StakeCredential, StakeCredentialType, display_collection,
    stake_credential_hash,
};
use amaru_ledger::store::{StoreError, columns::unsafe_decode};
use std::collections::BTreeSet;
use tracing::{Level, debug, instrument, warn};

/// Name prefixed used for storing Account -> DRep & DRep -> Account entries. UTF-8 encoding for
/// "dlg".
///
/// We leave one byte up, so that RocksDB will partition files into ~256 buckets. This allows to
/// reduce the burden of iterating over *all* dreps, but only a subset of those. On mainnet, we
/// count about 200k delegators, spread over 1.5k dreps. So iterating on the entire column can be a
/// little expensive.
///
/// With this extra byte, we should have amortised performances equivalent to iterating over ~1K
/// entries, which seems much more reasonable. Besides, that work is *bounded* anyway, because it
/// only happens during the 'bootstrapping phase of Conway'  (i.e. ProtocolVersion == 9).
pub const PREFIX: [u8; PREFIX_LEN - 1] = [0x64, 0x6c, 0x67];

/// Remember a previous binding Account <-> DRep; This allows to efficiently retrieve all
/// (past and present) delegations for a given DRep.
pub fn add<DB>(
    db: &Transaction<'_, DB>,
    rows: impl Iterator<Item = (StakeCredential, DRep, CertificatePointer)>,
) -> Result<(), StoreError> {
    for (delegator, drep, delegator_since) in rows {
        let drep = match drep {
            DRep::Key(hash) => StakeCredential::AddrKeyhash(hash),
            DRep::Script(hash) => StakeCredential::ScriptHash(hash),
            // We only need to keep track of this binding for registered dreps.
            DRep::Abstain | DRep::NoConfidence => continue,
        };

        // Check whether the DRep still exists and if the existing delegation is still ongoing.
        // This is, again, not intuitive but take the following scenario:
        //
        // - account delegates to Alice
        // - Alices unregisters
        // - Account delegates to Bob
        // - Alices re-registers
        // - Alice unregisters (again)
        //
        // When Alice first unregisters, we do not go and clean up all accounts from their existing
        // delegation to Alice, we instead record certificate pointers. So when the account
        // delegates to Bob, a binding Account -> Alice will percolate up to here.
        //
        // However, Alice unregistration has already happened and so, we must not record this
        // binding. It suffices to check whether Alice's last de-registration pointer, if any,
        // came after the delegation. In which case, we have technically already cleared up the
        // DRep relation due to the v9 bug. Note that:
        //
        // 1. We never reset the last de-registration pointer to None. Even when the DRep
        //    re-registered. So even the case where Alice only re-register prior to the delegation
        //    to Bob, we would not retain the link.
        //
        // 2. It doesn't matter if Alice re-registers and unregisters multiple times since any new
        //    unregistration will be after the first one, and thus, will not invalidate the
        //    inequality.
        if let Some(row) = dreps::get(db, &drep)? {
            if let Some(previous_deregistration) = row.previous_deregistration
                && previous_deregistration > delegator_since
            {
                debug!(
                    drep.type = %StakeCredentialType::from(&drep),
                    drep.hash = %stake_credential_hash(&drep),
                    delegator.type = %StakeCredentialType::from(&delegator),
                    delegator.hash = %stake_credential_hash(&delegator),
                    "drep has unregistered since delegation; skipping mapping",
                );
                continue;
            }

            db.put(new_key(&drep, &delegator), vec![])
                .map_err(|err| StoreError::Internal(err.into()))?;
        } else {
            warn!(
                drep.type = %StakeCredentialType::from(&drep),
                drep.hash = %stake_credential_hash(&drep),
                delegator.type = %StakeCredentialType::from(&delegator),
                delegator.hash = %stake_credential_hash(&delegator),
                "delegator was delegated to non-existing drep",
            );
        }
    }

    Ok(())
}

/// Forget about a past link between a drep and its delegator, if any.
pub fn remove<DB>(
    db: &Transaction<'_, DB>,
    drep: &DRep,
    delegator: &StakeCredential,
) -> Result<(), StoreError> {
    let drep = match drep {
        DRep::Key(hash) => StakeCredential::AddrKeyhash(*hash),
        DRep::Script(hash) => StakeCredential::ScriptHash(*hash),
        DRep::Abstain | DRep::NoConfidence => return Ok(()),
    };

    db.delete(new_key(&drep, delegator))
        .map_err(|err| StoreError::Internal(err.into()))
}

/// Forget about ALL bindings for a given drep, returning all known (past and present) delegations
/// for that drep.
#[instrument(
    level = Level::DEBUG,
    name = "dreps_delegations.remove",
    skip_all,
    fields(
        drep.hash = %stake_credential_hash(drep),
        drep.type = %StakeCredentialType::from(drep),
    )
)]
pub fn drop<DB>(
    db: &Transaction<'_, DB>,
    drep: &StakeCredential,
) -> Result<BTreeSet<StakeCredential>, StoreError> {
    let mut delegators = BTreeSet::new();

    let keys = iter_drep(db, drep)
        .map(|item| {
            item.map(|(key, delegator)| {
                delegators.insert(delegator);
                key
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| StoreError::Internal(err.into()))?;

    keys.iter()
        .try_for_each(|key| db.delete(key))
        .map_err(|err| StoreError::Internal(err.into()))?;

    if !delegators.is_empty() {
        debug!(
            delegators = format!(
                "[{}]",
                display_collection(
                    delegators
                        .iter()
                        .map(stake_credential_hash)
                        .collect::<Vec<_>>()
                )
            ),
            "clearing present (and past) delegators"
        )
    }

    Ok(delegators)
}

fn iter_drep<DB>(
    db: &Transaction<'_, DB>,
    drep: &StakeCredential,
) -> impl Iterator<Item = Result<(Box<[u8]>, StakeCredential), rocksdb::Error>> {
    let mut prefix = PREFIX.to_vec();
    prefix.extend_from_slice(as_value(drep).as_slice());

    // The invariant is preserved because the first 3 bytes are PREFIX, which is not all 0xFF.
    let upper_bound = into_next_prefix(prefix.clone());

    // NOTE(ROCKSDB):
    // We must set an explicit upper bound on this one, without what we end up sometimes yielding
    // values from other columns? I do not fully comprehend *why* (and more particularly, why it
    // doesn't happen when we iterate in other places);
    let mut opts = ReadOptions::default();
    opts.set_prefix_same_as_start(true);
    opts.set_iterate_upper_bound(upper_bound); // is exclusive

    db.iterator_opt(IteratorMode::From(&prefix[..], Direction::Forward), opts)
        .map(move |item| {
            item.map(|(key, _)| {
                let (_, right) = key.split_at(prefix.len());
                let delegator = unsafe_decode::<StakeCredential>(right.to_vec());
                (key, delegator)
            })
        })
}

/// Construct a drep delegation key from the drep and delegator respective stake credentials:
///
/// ┌──────────────────┬──────────────────┬───────────────────────┐
/// │ <PREFIX = "dlg"> │ <drep .bytes 30> │ <delegator .bytes 30> │
/// └──────────────────┴──────────────────┴───────────────────────┘
///
fn new_key(drep: &StakeCredential, delegator: &StakeCredential) -> Vec<u8> {
    let mut key = PREFIX.to_vec();
    key.extend_from_slice(as_value(drep).as_slice());
    key.extend_from_slice(as_value(delegator).as_slice());
    key
}

/// Smallest byte-string of equal length, strictly after 'bytes'. e.g.
///
/// ```rust
/// use amaru_stores::rocksdb::ledger::columns::dreps_delegations::into_next_prefix;
///
/// into_next_prefix(vec![0x12, 0x45, 0x34]) == vec![0x12, 0x45, 0x35];
/// into_next_prefix(vec![0x12, 0x45, 0xFF]) == vec![0x12, 0x46, 0xFF];
/// ```
///
/// Pre-condition: the given byte string is at least 2 bytes and not all 0xFF.
pub fn into_next_prefix(mut bytes: Vec<u8>) -> Vec<u8> {
    let mut i = bytes.len();

    debug_assert!(
        i > 1,
        "into_next_prefix called with empty or singleton bytes"
    );

    while i > 0 {
        i -= 1;
        if bytes[i] != 0xFF {
            bytes[i] += 1;
            break;
        }
    }

    debug_assert!(
        i > 0,
        "into_next_prefix called with saturated bytes (i.e. all 0xFF)"
    );

    bytes
}
