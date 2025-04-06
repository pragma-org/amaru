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

use crate::store::{columns::dreps, Snapshot, StoreError};
use amaru_kernel::{
    expect_stake_credential, network::EraHistory, Anchor, CertificatePointer, DRep, Epoch,
    Lovelace, ProtocolVersion, Slot, StakeCredential, TransactionPointer, DREP_EXPIRY,
    GOV_ACTION_LIFETIME,
};
use slot_arithmetic::TimeHorizonError;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
pub struct GovernanceSummary {
    pub dreps: BTreeMap<DRep, DRepState>,
    pub deposits: BTreeMap<StakeCredential, ProposalState>,
}

#[derive(Debug, serde::Serialize)]
pub struct DRepState {
    pub mandate: Option<Epoch>,
    pub metadata: Option<Anchor>,
    pub stake: Lovelace,
    #[serde(skip)]
    pub registered_at: CertificatePointer,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ProposalState {
    pub deposit: Lovelace,
    pub valid_until: Epoch,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("time horizon error: {0}")]
    TimeHorizonError(Slot, TimeHorizonError),
    #[error("store error: {0}")]
    StoreError(#[from] StoreError),
}

impl GovernanceSummary {
    pub fn new(db: &impl Snapshot, era_history: &EraHistory) -> Result<Self, Error> {
        let mut proposals = BTreeSet::new();

        // FIXME: filter out proposals that have been ratified
        let mut deposits = BTreeMap::new();

        db.iter_proposals()?
            .try_for_each(|(_, row)| -> Result<(), Error> {
                let epoch = era_history
                    .slot_to_epoch(row.proposed_in.transaction.slot)
                    .map_err(|e| Error::TimeHorizonError(row.proposed_in.transaction.slot, e))?;

                proposals.insert((row.proposed_in.transaction, epoch));

                // NOTE: Proposals are ratified with an epoch of delay always, so deposits count
                // towards the voting stake of DRep for an extra epoch following the proposal
                // expiry.
                if epoch <= row.valid_until + 1 {
                    deposits.insert(
                        expect_stake_credential(&row.proposal.reward_account),
                        ProposalState {
                            deposit: row.proposal.deposit,
                            valid_until: row.valid_until,
                        },
                    );
                }

                Ok(())
            })?;

        let mandate = drep_mandate_calculator(
            // FIXME: Obtain protocol version from arguments, passed from block header.
            (9, 0),
            GOV_ACTION_LIFETIME,
            DREP_EXPIRY,
            proposals,
        );

        let mut dreps = db
            .iter_dreps()?
            .map(
                |(
                    k,
                    dreps::Row {
                        registered_at,
                        last_interaction,
                        anchor,
                        ..
                    },
                )| {
                    let drep = match k {
                        StakeCredential::AddrKeyhash(hash) => DRep::Key(hash),
                        StakeCredential::ScriptHash(hash) => DRep::Script(hash),
                    };

                    let registration_slot = registered_at.transaction.slot;

                    Ok((
                        drep,
                        DRepState {
                            registered_at,
                            metadata: anchor,
                            mandate: Some(mandate(
                                // TODO: The map_err to include the slot as context shouldn't be
                                // necessary. Instead, the slot_arithmetic library should return
                                // better errors.
                                era_history
                                    .slot_to_epoch(registration_slot)
                                    .map_err(|e| Error::TimeHorizonError(registration_slot, e))?,
                                last_interaction,
                            )),
                            stake: 0, // NOTE: The actual stake is filled later when computing the
                                      // stake distribution.
                        },
                    ))
                },
            )
            .collect::<Result<BTreeMap<_, _>, Error>>()?;

        let default_drep_state = || DRepState {
            mandate: None,
            metadata: None,
            stake: 0,
            registered_at: CertificatePointer {
                transaction: TransactionPointer {
                    slot: Slot::from(0),
                    transaction_index: 0,
                },
                certificate_index: 0,
            },
        };

        dreps.insert(DRep::Abstain, default_drep_state());
        dreps.insert(DRep::NoConfidence, default_drep_state());

        Ok(GovernanceSummary { dreps, deposits })
    }
}

fn drep_mandate_calculator(
    protocol_version: ProtocolVersion,
    governance_action_lifetime: Epoch,
    drep_expiry: Epoch,
    proposals: BTreeSet<(TransactionPointer, Epoch)>,
) -> impl Fn(Epoch, Epoch) -> u64 {
    // A set containing all overlapping activity periods of all proposals. Might contain disjoint periods.
    // e.g.
    //
    // Considering a proposal created at epoch 163, it is valid until epoch 163 + GOV_ACTION_LIFETIME
    //
    // for epochs 163 and 165, with GOV_ACTION_LIFETIME = 6, proposals_activity_periods would equal
    //   [163, 164, 165, 166, 167, 168, 169, 170, 171]
    //
    // for epochs 163 and 172, with GOV_ACTION_LIFETIME = 6, proposals_activity_periods would equal
    //   [163, 164, 165, 166, 167, 168, 169, 172, 173, 174, 175, 176, 177, 178]
    let proposals_activity_periods = proposals
        .iter()
        .flat_map(|(_pointer, start)| {
            (*start..=*start + governance_action_lifetime).collect::<BTreeSet<_>>()
        })
        .collect::<BTreeSet<Epoch>>();

    let most_recent_epoch = proposals_activity_periods.iter().max().copied();

    move |registered_at, last_interaction| {
        // Each epoch with no active proposals increase the mandate by 1
        let dormant_epochs =
            BTreeSet::from_iter(last_interaction..=most_recent_epoch.unwrap_or(last_interaction)) // Total period considered for this DRep
                .difference(&proposals_activity_periods)
                .count() as u64;

        last_interaction + drep_expiry + dormant_epochs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amaru_kernel::network::{Bound, EraParams, NetworkName, Summary};
    use std::sync::LazyLock;
    use test_case::test_case;

    const VERSION_9: ProtocolVersion = (9, 0);
    const VERSION_10: ProtocolVersion = (10, 0);

    static ERA_HISTORY: LazyLock<EraHistory> = LazyLock::new(|| EraHistory {
        eras: vec![Summary {
            start: Bound {
                time_ms: 0,
                slot: From::from(0),
                epoch: 0,
            },
            end: Bound {
                time_ms: 1000000,
                slot: From::from(1000),
                epoch: 100,
            },
            params: EraParams {
                epoch_size_slots: 10,
                slot_length: 1000,
            },
        }],
    });

    fn ptr(slot: u64, transaction_index: usize) -> (TransactionPointer, Epoch) {
        let slot = Slot::from(slot);
        (
            TransactionPointer {
                slot,
                transaction_index,
            },
            ERA_HISTORY.slot_to_epoch(slot).unwrap(),
        )
    }

    #[test_case(
        VERSION_10, 3, 10, vec![ptr(85, 0)], 8, 8 => 18;
        "VERSION=10 no dormant period, no interaction"
    )]
    #[test_case(
        VERSION_10, 3, 10, vec![ptr(85, 0)], 8, 9 => 19;
        "VERSION=10 no dormant period, one recent interaction"
    )]
    #[test_case(
        VERSION_10, 3, 10, vec![ptr(85, 0), ptr(145, 0)], 5, 9 => 21;
        "VERSION=10 single 2-epoch dormant period, one old interaction"
    )]
    #[test_case(
        VERSION_10, 3, 10, vec![ptr(85, 0), ptr(135, 0), ptr(205, 0)], 13, 13 => 26;
        "VERSION=10 4-epoch cumulative dormant period, no interaction, early registration"
    )]
    #[test_case(
        VERSION_10, 3, 10, vec![ptr(85, 0), ptr(135, 0), ptr(205, 0)], 20, 20 => 30;
        "VERSION=10 4-epoch cumulative dormant period, no interaction, late registration"
    )]
    #[test_case(
        VERSION_10, 3, 10, vec![ptr(85, 0), ptr(135, 0), ptr(205, 0)], 12, 15 => 28;
        "VERSION=10 4-epoch cumulative dormant period, some interactions"
    )]
    #[test_case(
        VERSION_10, 3, 10, vec![ptr(85, 0), ptr(125, 0), ptr(155, 0)], 8, 13 => 23;
        "VERSION=10 no dormant period, some interactions"
    )]
    fn test_drep_mandate(
        protocol_version: ProtocolVersion,
        governance_action_lifetime: Epoch,
        drep_expiry: Epoch,
        proposals: Vec<(TransactionPointer, Epoch)>,
        registered_at: Epoch,
        last_interaction: Epoch,
    ) -> Epoch {
        drep_mandate_calculator(
            protocol_version,
            governance_action_lifetime,
            drep_expiry,
            proposals.into_iter().collect::<BTreeSet<_>>(),
        )(registered_at, last_interaction)
    }
}
