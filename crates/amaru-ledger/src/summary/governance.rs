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

mod backward_compatibility;

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
    #[serde(skip)]
    pub previous_deregistration: Option<CertificatePointer>,
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
    pub fn new(
        db: &impl Snapshot,
        protocol_version: ProtocolVersion,
        era_history: &EraHistory,
    ) -> Result<Self, Error> {
        let current_epoch = db.epoch();

        let mut proposals = BTreeSet::new();

        // FIXME: filter out proposals that have been ratified
        let mut deposits = BTreeMap::new();

        db.iter_proposals()?
            .try_for_each(|(_, row)| -> Result<(), Error> {
                let epoch = era_history
                    .slot_to_epoch(row.proposed_in.transaction.slot)
                    .map_err(|e| Error::TimeHorizonError(row.proposed_in.transaction.slot, e))?;

                proposals.insert((row.proposed_in.transaction, epoch));

                // Proposals are ratified with an epoch of delay always, so deposits count towards
                // the voting stake of DRep for an extra epoch following the proposal expiry.
                if current_epoch <= row.valid_until + 1 {
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
            protocol_version,
            GOV_ACTION_LIFETIME,
            DREP_EXPIRY,
            era_history,
            current_epoch,
            proposals,
        );

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
                )| {
                    let is_default = registered_at == &CertificatePointer::default();
                    Some(registered_at) > previous_deregistration.as_ref()
                        // This second condition stems from how we import snapshots from incomplete
                        // data: we do not know when a drep registers when it's already in a
                        // snapshot. The default is filled with zeroes, and thus, will always be
                        // smaller than any registration certificate.
                        || is_default && previous_deregistration.is_some()
                },
            )
            .map(
                |(
                    k,
                    dreps::Row {
                        registered_at,
                        previous_deregistration,
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
                            previous_deregistration,
                            metadata: anchor,
                            mandate: Some(mandate(
                                // TODO: The map_err to include the slot as context shouldn't be
                                // necessary. Instead, the slot_arithmetic library should return
                                // better errors.
                                (
                                    registration_slot,
                                    era_history.slot_to_epoch(registration_slot).map_err(|e| {
                                        Error::TimeHorizonError(registration_slot, e)
                                    })?,
                                ),
                                last_interaction,
                            )),
                            // The actual stake is filled later when computing the stake distribution.
                            stake: 0,
                        },
                    ))
                },
            )
            .collect::<Result<BTreeMap<_, _>, Error>>()?;

        let default_protocol_drep = || DRepState {
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
            previous_deregistration: None,
        };

        dreps.insert(DRep::Abstain, default_protocol_drep());
        dreps.insert(DRep::NoConfidence, default_protocol_drep());

        Ok(GovernanceSummary { dreps, deposits })
    }
}

/// Compute the mandate (e.g. expiry epoch) of a DRep based on protocol parameter and past
/// proposals. It works like a deadman-switch, where each action from a DRep resets the counter and
/// push back its expiry. Additionally, each epoch with no active proposals increase the mandate by
/// one.
///
/// Besides, the behaviour around registration has slightly changed between verion 9 and version 10
/// of the protocol.
///
/// - In version 9, dreps registering during a dormant period^1 (with epochs containing no proposals
///   whatsoever) would be granted extra expiry time corresponding to the current number of dormant
///   epoch.
///
/// - In version 10, freshly register dreps will only be granted their "drep_expiry" from the
///   moment they register.
///
/// NOTE[^1]: About dormant period
///
///   Intuitively, a dormant period is a sequence of consecutive epochs with no proposals. However,
///   the intuition isn't quite right when it comes to what the protocol *actually does*. In fact,
///   we assess dormant epochs at the epoch boundary, after ratifying and/or expirying proposals. In
///   case where there are no proposals left at the epoch boundary, then the next epoch is considered
///   dormant.
///
///   Let's see a couple of examples, with the following base hypotheses:
///
///   - We are in epoch 16.
///   - There's no proposal whatsoever before epoch 10.
///   - Proposals' lifetime is 2 epochs
///
///   ╔══════ Scenario 1: a single proposal in 10.
///   ║
///   ║     <--------------->
///   ║     ╿
///   ║ ━━━━┷━╋━━━━━━╋━━━━━━╋━━━━━━╋━━━━━━╋━━━━━━╋━━━?
///   ║   10     11     12     13     14     15     16
///   ║
///   ║ Dormant epochs: 10, 13, 14, 15, 16
///   ╚═════════════════════════════════════════════════
///
///   ╔══════ Scenario 2: two proposals in 10 and 12
///   ║
///   ║     <╌--------------------->
///   ║     ╿          ╿
///   ║ ━━━━┷━╋━━━━━━╋━┷━━━━╋━━━━━━╋━━━━━━╋━━━━━━╋━━━?
///   ║   10     11     12     13     14     15     16
///   ║
///   ║ Dormant epochs: 10, 14, 15, 16
///   ╚═════════════════════════════════════════════════
///
///   ╔══════ Scenario 3: two proposals in 10 and 13
///   ║
///   ║     <╌--------------><╌╌╌╌╌-------------->
///   ║     ╿                ╿
///   ║ ━━━━┷━╋━━━━━━╋━━━━━━╋┷━━━━━╋━━━━━━╋━━━━━━╋━━━?
///   ║   10     11     12     13     14     15     16
///   ║
///   ║ Dormant epochs: 10, 13, 16
///   ╚═════════════════════════════════════════════════
///
fn drep_mandate_calculator(
    protocol_version: ProtocolVersion,
    governance_action_lifetime: Epoch,
    drep_expiry: Epoch,
    era_history: &EraHistory,
    current_epoch: Epoch,
    proposals: BTreeSet<(TransactionPointer, Epoch)>,
) -> Box<dyn Fn((Slot, Epoch), Epoch) -> Epoch> {
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
            // Notice '+1' here on the lower bound. The epoch in which a proposal is submitted
            // does not count towards a period of activity because we evaluate whether an epoch is
            // dormant at the epoch boundary. If no proposals are active at the epoch boundary, the
            // epoch is considered dormant.
            //
            // While this is slightly odd, it can be explained by considering how one could submit
            // a proposal at the very end of epoch with little to no time for DReps to vote. An
            // epoch with no proposal but on the last slot is arguably dormant. But as a
            // consequence, we may also label as dormant epochs with proposals submitted on the
            // very first slot too.
            (*start + 1..=*start + governance_action_lifetime).collect::<BTreeSet<_>>()
        })
        .collect::<BTreeSet<Epoch>>();

    // Pre-calculate all epochs, so that need not to re-allocate memory for all DReps.
    //
    // FIXME: This initial epoch should be bound to the oldest epoch known of Amaru.
    // We cannot access data older than that *anyway*.
    let from_epoch = era_history
        .eras
        .last()
        .map(|summary| summary.start.epoch)
        .unwrap_or_default();

    let all_epochs = BTreeSet::from_iter(from_epoch..=current_epoch);

    let era_first_epoch = era_history.era_first_epoch(current_epoch);

    let v10_onwards = Box::new(
        move |registered_at: (Slot, Epoch), last_interaction: Epoch| -> Epoch {
            let active_epochs = proposals_activity_periods
                .iter()
                // Exclude any period prior to the drep registration. They shouldn't count towards
                // the dormant epochs number, since the DRep simply didn't exist back then.
                .filter(|epoch| *epoch >= &registered_at.1)
                // Always consider the registration & last interaction epoch as active epochs so
                // that if the drep is registered/updated in the middle of a dormant period, it
                // only counts from the epoch following the event.
                .chain(vec![&registered_at.1, &last_interaction])
                .collect::<BTreeSet<_>>();

            debug_assert!(
                last_interaction <= current_epoch,
                "drep recorded last interaction ({last_interaction}) is beyond the most recent epoch ({current_epoch})?"
            );

            let dormant_epochs = all_epochs
                .range(last_interaction..=current_epoch)
                .filter(|epoch| !active_epochs.contains(epoch))
                .count() as u64;

            last_interaction + drep_expiry + dormant_epochs
        },
    );

    let major_version = protocol_version.0;

    if major_version <= 9 {
        return Box::new(
            move |registered_at: (Slot, Epoch), last_interaction: Epoch| -> Epoch {
                let bonus_bug_dormant = backward_compatibility::drep_bonus_mandate(
                    governance_action_lifetime,
                    &proposals,
                    registered_at,
                );

                let bonus_first_epoch = if Ok(current_epoch) == era_first_epoch {
                    1
                } else {
                    0
                };

                bonus_first_epoch + bonus_bug_dormant + v10_onwards(registered_at, last_interaction)
            },
        );
    }

    v10_onwards
}

#[cfg(test)]
mod tests {
    use super::*;
    use amaru_kernel::{PROTOCOL_VERSION_10, PROTOCOL_VERSION_9};
    use backward_compatibility::tests::{ptr, ERA_HISTORY};
    use test_case::test_case;
    use EpochResult::*;

    #[derive(Debug, PartialEq)]
    enum EpochResult {
        Consistent(Epoch),
        Inconsistent { v9: Epoch, v10: Epoch },
    }

    fn test_drep_mandate(
        governance_action_lifetime: u64,
        drep_expiry: u64,
        proposals: Vec<(TransactionPointer, Epoch)>,
        registered_at: u64,
        last_interaction: Option<Epoch>,
        current_epoch: Epoch,
    ) -> EpochResult {
        let registration_slot = Slot::from(registered_at);
        let registration_epoch = ERA_HISTORY.slot_to_epoch(registration_slot).unwrap();
        let proposals = proposals.into_iter().collect::<BTreeSet<_>>();

        let test_with = |protocol_version| {
            drep_mandate_calculator(
                protocol_version,
                governance_action_lifetime,
                drep_expiry,
                &ERA_HISTORY,
                current_epoch,
                proposals.clone(),
            )(
                (registration_slot, registration_epoch),
                last_interaction.unwrap_or(registration_epoch),
            )
        };

        let v9 = test_with(PROTOCOL_VERSION_9);

        let v10 = test_with(PROTOCOL_VERSION_10);

        if v9 == v10 {
            Consistent(v10)
        } else {
            Inconsistent { v9, v10 }
        }
    }

    // Scenario:
    //   - no past dormant period
    //   - dormant starting from epoch 12
    //
    //     proposal
    //       |
    //  |----x----|---------|---------|------>
    // 80   85   90        100       110
    //
    #[test_case( 84,    None,  8 => Consistent(18))]
    #[test_case( 84, Some(9),  9 => Consistent(19))]
    #[test_case( 85,    None,  8 => Consistent(18))]
    #[test_case( 85, Some(9),  9 => Consistent(19))]
    #[test_case( 86,    None,  8 => Consistent(18))]
    #[test_case( 86, Some(9),  9 => Consistent(19))]
    #[test_case(125,    None, 12 => Inconsistent { v9: 23, v10: 22 })]
    #[test_case(125,    None, 13 => Inconsistent { v9: 24, v10: 23 })]
    #[test_case(135,    None, 13 => Inconsistent { v9: 25, v10: 23 })]
    fn test_drep_mandate_no_dormant_period(
        registered_at: u64,
        last_interaction: Option<Epoch>,
        current_epoch: Epoch,
    ) -> EpochResult {
        test_drep_mandate(
            3,                // governance_action_lifetime
            10,               // drep_expiry
            vec![ptr(85, 0)], // proposals
            registered_at,
            last_interaction,
            current_epoch,
        )
    }

    // Scenario:
    //   - 1 dormant period (epoch 12-13)
    //
    //      proposal               proposal
    //        |                      |
    //  |-----x------|--...---|------x-----|
    // 80    85     90       130    135   140
    //
    #[test_case( 88,     None, 11 => Consistent(18))]
    #[test_case( 88,     None, 12 => Consistent(19))]
    #[test_case( 88,     None, 13 => Consistent(20))]
    #[test_case( 88, Some(10), 11 => Consistent(20))]
    #[test_case( 88, Some(10), 12 => Consistent(21))]
    #[test_case( 88, Some(10), 13 => Consistent(22))]
    #[test_case( 88, Some(10), 14 => Consistent(22))]
    #[test_case(115,     None, 11 => Consistent(21))]
    #[test_case(115,     None, 13 => Consistent(23))]
    #[test_case(115, Some(13), 13 => Consistent(23))]
    #[test_case(125,     None, 13 => Inconsistent{ v9: 24, v10: 23 })]
    #[test_case(134,     None, 13 => Inconsistent{ v9: 25, v10: 23 })]
    #[test_case(135,     None, 13 => Inconsistent{ v9: 25, v10: 23 })]
    #[test_case(136,     None, 13 => Consistent(23))]
    fn test_drep_mandate_single_dormant_period(
        registered_at: u64,
        last_interaction: Option<Epoch>,
        current_epoch: Epoch,
    ) -> EpochResult {
        test_drep_mandate(
            3,                             // governance_action_lifetime
            10,                            // drep_expiry
            vec![ptr(85, 0), ptr(135, 0)], // proposals
            registered_at,
            last_interaction,
            current_epoch,
        )
    }

    // Scenario:
    //   - 2 dormant periods
    //     - epoch 4-5-6
    //     - epoch 10-11
    //
    //      proposal              proposal            proposal
    //        |                     |                   |
    //  |-----x------|--...--|------x-----|--...--|-----x------>
    //  0     5     10       60    65   70       110   115
    //
    #[test_case(  1,    None, 11 => Consistent(15))]
    #[test_case( 64,    None,  8 => Inconsistent{ v9: 19, v10: 16 })]
    #[test_case( 64,    None, 10 => Inconsistent{ v9: 20, v10: 17 })]
    #[test_case( 64,    None, 11 => Inconsistent{ v9: 21, v10: 18 })]
    #[test_case( 65,    None, 11 => Inconsistent{ v9: 21, v10: 18 })]
    #[test_case( 65, Some(8), 11 => Inconsistent{ v9: 23, v10: 20 })]
    #[test_case( 66,    None, 11 => Consistent(18))]
    #[test_case( 66, Some(8), 10 => Consistent(19))]
    #[test_case( 66, Some(8), 11 => Consistent(20))]
    #[test_case(114,    None, 11 => Inconsistent{ v9: 23, v10: 21 })]
    #[test_case(115,    None, 11 => Inconsistent{ v9: 23, v10: 21 })]
    #[test_case(116,    None, 11 => Consistent(21))]
    #[test_case(140,    None, 14 => Consistent(24))]
    #[test_case(150,    None, 15 => Inconsistent{ v9: 26, v10: 25 })]
    fn test_drep_mandate_multiple_dormant_periods(
        registered_at: u64,
        last_interaction: Option<Epoch>,
        current_epoch: Epoch,
    ) -> EpochResult {
        test_drep_mandate(
            3,                                        // governance_action_lifetime
            10,                                       // drep_expiry
            vec![ptr(5, 0), ptr(65, 0), ptr(115, 0)], // proposals
            registered_at,
            last_interaction,
            current_epoch,
        )
    }
}
