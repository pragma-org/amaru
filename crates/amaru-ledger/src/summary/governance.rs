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

use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Deref,
};

use amaru_kernel::{
    Anchor, CertificatePointer, ComparableProposalId, DRep, Epoch, EraHistory, EraHistoryError, Lovelace,
    RatificationStatus, Slot, StakeCredential, TransactionPointer, anchor, expect_stake_credential,
};

use crate::{
    epoch_transition::GovernanceActivity,
    store::{Snapshot, StoreError, columns::dreps},
};

#[derive(Debug)]
pub struct GovernanceSummary {
    pub dreps: BTreeMap<DRep, DRepState>,
    pub dreps_deposits: BTreeMap<StakeCredential, Lovelace>,
    pub pools_deposits: BTreeMap<StakeCredential, Lovelace>,
}

#[derive(Debug, serde::Serialize)]
#[cfg_attr(test, derive(Clone))]
pub struct DRepState {
    #[serde(serialize_with = "anchor::serialize")]
    pub metadata: Option<Anchor>,
    pub valid_until: Option<Epoch>,
    pub voting_stake: Lovelace,
    #[serde(skip)]
    pub registered_at: CertificatePointer,
}

impl DRepState {
    pub fn is_active(&self, epoch: Epoch) -> bool {
        self.valid_until.is_none() || self.valid_until > Some(epoch)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ProposalState {
    pub deposit: Lovelace,
    pub valid_until: Epoch,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("time horizon error: {0}")]
    EraHistoryError(Slot, EraHistoryError),
    #[error("store error: {0}")]
    StoreError(#[from] StoreError),
}

impl GovernanceSummary {
    pub fn new(db: &impl Snapshot, era_history: &EraHistory) -> Result<Self, Error> {
        let current_epoch = db.epoch();

        let GovernanceActivity { consecutive_dormant_epochs } = db.governance_activity()?;

        let mut proposals = BTreeSet::new();
        let mut dreps_deposits: BTreeMap<StakeCredential, Lovelace> = BTreeMap::new();
        let mut pools_deposits: BTreeMap<StakeCredential, Lovelace> = BTreeMap::new();

        let recently_pruned_proposals: BTreeMap<ComparableProposalId, RatificationStatus> =
            db.iter_recently_pruned_proposals()?.collect();

        db.iter_proposals()?.try_for_each(|(proposal_id, row)| -> Result<(), Error> {
            let proposed_in = era_history
                .slot_to_epoch_unchecked_horizon(row.proposed_in.transaction.slot)
                .map_err(|e| Error::EraHistoryError(row.proposed_in.transaction.slot, e))?;

            proposals.insert((row.proposed_in.transaction, proposed_in));

            // Proposals are ratified with an epoch of delay always, so deposits count towards
            // the voting stake for an extra epoch following the proposal expiry.
            if current_epoch <= row.valid_until + 1 {
                let stake_credential = expect_stake_credential(&row.proposal.reward_account);
                let deposit: u64 = row.proposal.deposit;
                let recently_pruned = recently_pruned_proposals.get(&proposal_id);

                // NOTE: Pool voting stake distribution after proposal pruning
                //
                // The stake distribution used for computing pools voting power is taken before
                // refunds or withdrawals are processed. Yet the stake distribution calculation
                // begins after proposals have been ratified/pruned.
                //
                // This means that when proposals are pruned (due to ratification, expiry or a
                // dependency thereof), the stake corresponding to the associated proposal's deposit
                // or withdrawals momentarily stops contributing towards the pools voting stake; and
                // magically re-appear in the next epoch.
                //
                // Interestingly, this is only true for stake pools, not for DReps. The DReps voting
                // stake correctly uses the stake distribution after the refunds / withdrawals have
                // been processed; so this gap is only observed for stake pools.
                if current_epoch <= row.valid_until && recently_pruned.is_none() {
                    pools_deposits
                        .entry(stake_credential.clone())
                        .and_modify(|total| *total += deposit)
                        .or_insert(deposit);
                }

                dreps_deposits.entry(stake_credential).and_modify(|total| *total += deposit).or_insert(deposit);

                // NOTE: Ratified withdrawals immediately count towards DRep voting stake
                //
                // This really is a weird edge case but, the stake distribution to compute the DRep
                // voting power is taken just after the deposits and withdrawals payouts. So a
                // withdrawal that was just ratified could contribute towards the DRep stake
                // distribution.
                //
                // What makes it weird (beyond the fact that it's using stuff happening in the
                // future), is that the constitution mostly prevents this since it (currently)
                // requires that target addresses are not delegated to a registered drep. So while
                // this never happens *in practice*, it can happen in theory and is not even
                // technically complicated. So we must get this right.
                if let Some(RatificationStatus::Ratified) = recently_pruned {
                    use amaru_kernel::GovernanceAction::*;
                    match row.proposal.gov_action {
                        TreasuryWithdrawals(withdrawals, _) => {
                            for (bytes, withdrawal) in withdrawals.deref() {
                                dreps_deposits
                                    .entry(expect_stake_credential(bytes))
                                    .and_modify(|total| *total += withdrawal)
                                    .or_insert(*withdrawal);
                            }
                        }
                        ParameterChange(..)
                        | HardForkInitiation(..)
                        | NoConfidence(..)
                        | UpdateCommittee(..)
                        | NewConstitution(..)
                        | Information => (),
                    }
                }
            }

            Ok(())
        })?;

        let mut dreps = db
            .iter_dreps()?
            .map(|(k, dreps::Row { registered_at, valid_until, anchor, .. })| {
                let drep = match k {
                    StakeCredential::AddrKeyhash(hash) => DRep::Key(hash),
                    StakeCredential::ScriptHash(hash) => DRep::Script(hash),
                };

                Ok((
                    drep,
                    DRepState {
                        registered_at,
                        metadata: anchor,
                        valid_until: Some(valid_until + consecutive_dormant_epochs as u64),
                        // The actual stake is filled later when computing the stake distribution.
                        voting_stake: 0,
                    },
                ))
            })
            .collect::<Result<BTreeMap<_, _>, Error>>()?;

        let default_protocol_drep = || DRepState {
            valid_until: None,
            metadata: None,
            voting_stake: 0,
            registered_at: CertificatePointer {
                transaction: TransactionPointer { slot: Slot::from(0), transaction_index: 0 },
                certificate_index: 0,
            },
        };

        dreps.insert(DRep::Abstain, default_protocol_drep());
        dreps.insert(DRep::NoConfidence, default_protocol_drep());

        Ok(GovernanceSummary { dreps, dreps_deposits, pools_deposits })
    }
}
