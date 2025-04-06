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
    expect_stake_credential, Anchor, CertificatePointer, DRep, Epoch, Lovelace, StakeCredential,
    TransactionPointer, DREP_EXPIRY, GOV_ACTION_LIFETIME,
};
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

impl GovernanceSummary {
    pub fn new(db: &impl Snapshot) -> Result<Self, StoreError> {
        let epoch = db.epoch();

        let mut all_proposals_epochs = BTreeSet::new();

        // FIXME: filter out proposals that have been ratified
        let deposits = db
            .iter_proposals()?
            .filter_map(|(_, row)| {
                all_proposals_epochs.insert(row.proposed_in);
                // NOTE: Proposals are ratified with an epoch of delay always, so deposits count
                // towards the voting stake of DRep for an extra epoch following the proposal
                // expiry.
                if epoch <= row.valid_until + 1 {
                    Some((
                        expect_stake_credential(&row.proposal.reward_account),
                        ProposalState {
                            deposit: row.proposal.deposit,
                            valid_until: row.valid_until,
                        },
                    ))
                } else {
                    None
                }
            })
            .collect::<BTreeMap<_, _>>();

        let mandate =
            drep_mandate_calculator(GOV_ACTION_LIFETIME, DREP_EXPIRY, all_proposals_epochs);

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

                    (
                        drep,
                        DRepState {
                            registered_at,
                            metadata: anchor,
                            mandate: Some(mandate(last_interaction)),
                            stake: 0, // NOTE: The actual stake is filled later when computing the
                                      // stake distribution.
                        },
                    )
                },
            )
            .collect::<BTreeMap<_, _>>();

        let default_drep_state = || DRepState {
            mandate: None,
            metadata: None,
            stake: 0,
            registered_at: CertificatePointer {
                transaction_pointer: TransactionPointer {
                    slot: From::from(0),
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
    governance_action_lifetime: Epoch,
    drep_expiry: Epoch,
    proposals: BTreeSet<Epoch>,
) -> impl Fn(Epoch) -> u64 {
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
        .flat_map(|&value| (value..=value + governance_action_lifetime).collect::<BTreeSet<_>>())
        .collect::<BTreeSet<Epoch>>();

    let most_recent_epoch = proposals_activity_periods.iter().max().copied();

    move |last_interaction| {
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
    use test_case::test_case;

    #[test_case(
        3, 10, vec![168], 0 => 178;
        "no dormant period, no interaction"
    )]
    #[test_case(
        3, 10, vec![168], 169 => 179;
        "no dormant period, one interaction"
    )]
    #[test_case(
        3, 10, vec![168, 174], 169 => 181;
        "single 2-epoch dormant period, one old interaction"
    )]
    #[test_case(
        3, 10, vec![168, 174], 174 => 184;
        "single 2-epoch dormant period, one recent interaction"
    )]
    #[test_case(
        3, 10, vec![168, 173, 180], 168 => 182;
        "4-epoch cumulative dormant period, no interaction"
    )]
    #[test_case(
        3, 10, vec![168, 173, 180], 175 => 188;
        "4-epoch cumulative dormant period, some interactions"
    )]
    #[test_case(
        3, 10, vec![168, 172, 175], 173 => 183;
        "no dormant period, some interactions"
    )]
    fn test_drep_mandate(
        governance_action_lifetime: Epoch,
        drep_expiry: Epoch,
        proposals: Vec<Epoch>,
        last_interaction: Epoch,
    ) -> Epoch {
        drep_mandate_calculator(
            governance_action_lifetime,
            drep_expiry,
            proposals.into_iter().collect::<BTreeSet<_>>(),
        )(last_interaction)
    }
}
