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

use crate::store::{GovernanceActivity, Snapshot, StoreError, columns::dreps};
use amaru_kernel::{
    Anchor, CertificatePointer, DRep, Lovelace, Slot, StakeCredential, TransactionPointer,
    expect_stake_credential, network::EraHistory,
};
use amaru_slot_arithmetic::{Epoch, EraHistoryError};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
pub struct GovernanceSummary {
    pub dreps: BTreeMap<DRep, DRepState>,
    pub deposits: BTreeMap<StakeCredential, Vec<ProposalState>>,
}

#[derive(Debug, serde::Serialize)]
#[cfg_attr(test, derive(Clone))]
pub struct DRepState {
    #[serde(rename(serialize = "mandate"))]
    pub valid_until: Option<Epoch>,
    pub metadata: Option<Anchor>,
    pub stake: Lovelace,
    #[serde(skip)]
    pub registered_at: CertificatePointer,
    #[serde(skip)]
    pub previous_deregistration: Option<CertificatePointer>,
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

        let mut proposals = BTreeSet::new();

        let mut deposits = BTreeMap::new();

        let GovernanceActivity {
            consecutive_dormant_epochs,
        } = db.governance_activity()?;

        db.iter_proposals()?
            .try_for_each(|(_, row)| -> Result<(), Error> {
                #[allow(clippy::disallowed_methods)]
                let epoch = era_history
                    .slot_to_epoch_unchecked_horizon(row.proposed_in.transaction.slot)
                    .map_err(|e| Error::EraHistoryError(row.proposed_in.transaction.slot, e))?;

                proposals.insert((row.proposed_in.transaction, epoch));

                // Proposals are ratified with an epoch of delay always, so deposits count towards
                // the voting stake of DRep for an extra epoch following the proposal expiry.
                if current_epoch <= row.valid_until + 1 {
                    let proposal = || ProposalState {
                        deposit: row.proposal.deposit,
                        valid_until: row.valid_until,
                    };

                    deposits
                        .entry(expect_stake_credential(&row.proposal.reward_account))
                        .and_modify(|proposals: &mut Vec<ProposalState>| proposals.push(proposal()))
                        .or_insert_with(|| vec![proposal()]);
                }

                Ok(())
            })?;

        let mut dreps = db
            .iter_dreps()?
            .filter(
                |(
                    _,
                    dreps::Row {
                        registered_at,
                        previous_deregistration,
                        ..
                    },
                )| { Some(registered_at) > previous_deregistration.as_ref() },
            )
            .map(
                |(
                    k,
                    dreps::Row {
                        registered_at,
                        previous_deregistration,
                        valid_until,
                        anchor,
                        ..
                    },
                )| {
                    let drep = match k {
                        StakeCredential::AddrKeyhash(hash) => DRep::Key(hash),
                        StakeCredential::ScriptHash(hash) => DRep::Script(hash),
                    };

                    Ok((
                        drep,
                        DRepState {
                            registered_at,
                            previous_deregistration,
                            metadata: anchor,
                            valid_until: Some(valid_until + consecutive_dormant_epochs as u64),
                            // The actual stake is filled later when computing the stake distribution.
                            stake: 0,
                        },
                    ))
                },
            )
            .collect::<Result<BTreeMap<_, _>, Error>>()?;

        let default_protocol_drep = || DRepState {
            valid_until: None,
            metadata: None,
            stake: 0,
            registered_at: CertificatePointer {
                transaction: TransactionPointer {
                    slot: Slot::from(0),
                    transaction_index: 0,
                },
                certificate_index: 0,
            },
            previous_deregistration: None,
        };

        dreps.insert(DRep::Abstain, default_protocol_drep());
        dreps.insert(DRep::NoConfidence, default_protocol_drep());

        Ok(GovernanceSummary { dreps, deposits })
    }
}
