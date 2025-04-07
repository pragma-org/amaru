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
            // FIXME: Obtain protocol version from arguments, passed from block header.
            (9, 0),
            GOV_ACTION_LIFETIME,
            DREP_EXPIRY,
            current_epoch,
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

/// Compute the mandate (e.g. expiry epoch) of a DRep based on protocol parameter and past
/// proposals. It works like a deadman-switch, where each action from a DRep resets the counter and
/// push back its expiry. Additionally, each epoch with no active proposals increase the mandate by
/// one.
///
/// Besides, the behaviour around registration has slightl change between verion 9 and version 10
/// of the protocol.
///
/// - In version 9, dreps registering during a dormant period (with epochs containing no proposals
///   whatsoever) would be granted extra expiry time corresponding to the current number of dormant
///   epoch.
///
/// - In version 10, freshly register dreps will only be granted their "drep_expiry" from the
///   moment they register.
fn drep_mandate_calculator(
    protocol_version: ProtocolVersion,
    governance_action_lifetime: Epoch,
    drep_expiry: Epoch,
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
            (*start + 1..=*start + governance_action_lifetime).collect::<BTreeSet<_>>()
        })
        .collect::<BTreeSet<Epoch>>();

    let v10_onwards = Box::new(
        move |registered_at: (Slot, Epoch), last_interaction: Epoch| -> Epoch {
            let dormant_epochs = BTreeSet::from_iter(last_interaction..=current_epoch)
                .iter()
                .collect::<BTreeSet<&u64>>()
                .difference(
                    &proposals_activity_periods
                        .iter()
                        // Exclude any period prior to the drep registration. They shouldn't count towards
                        // the dormant epochs number, since the DRep simply didn't exist back then.
                        .filter(|epoch| *epoch >= &registered_at.1)
                        // Always consider the registration epoch as an active epoch so that if the
                        // drep is registered in the middle of a dormant period, it only counts from
                        // the epoch following the registration.
                        .chain(vec![&registered_at.1])
                        .collect(),
                )
                .count() as u64;

            last_interaction + drep_expiry + dormant_epochs
        },
    );

    let major_version = protocol_version.0;

    if major_version <= 9 {
        return Box::new(
            move |registered_at: (Slot, Epoch), last_interaction: Epoch| -> Epoch {
                drep_bonus_mandate(governance_action_lifetime, &proposals, registered_at)
                    + v10_onwards(registered_at, last_interaction)
            },
        );
    }

    v10_onwards
}

// In Version 9, the number of dormant epochs depends on the moment the DRep registers *within the
// epoch*.
//
// The original Haskell implementation would keep a counter of dormant epoch in-memory, incremented
// at epoch boundaries for epochs without proposals. Besides, and unlike Amaru, the Haskell
// implementation stores the actual expiry epoch for each DRep instead of their registration date
// and last interaction.
//
// The expiration date is then updated for each DRep interations, and "periodically" when the
// dormant counter is greater than 0. "Periodically" is in fact, **every time a transaction with
// one or more new proposal is encountered**. So we need to figure out the _last dormant epoch
// count_ that could be relevant to the DRep.
fn drep_bonus_mandate(
    governance_action_lifetime: u64,
    proposals: &BTreeSet<(TransactionPointer, Epoch)>,
    (registered_at, registered_in): (Slot, Epoch),
) -> Epoch {
    let proposal_until_registration = proposals
        .iter()
        .filter(|(_, proposed_in)| proposed_in <= &registered_in)
        .collect();

    match last_dormant_period(
        governance_action_lifetime,
        registered_in,
        proposal_until_registration,
    ) {
        LastDormantEpoch::Empty { .. } | LastDormantEpoch::None { .. } => 0,

        // The dormant epoch is ongoing, so always add the bonus to new DReps;
        LastDormantEpoch::Last {
            ending_at: None,
            length,
        } => length,

        // The dormant epoch has ended, so only add the bonus if the drep registered *before* it
        // ended.
        //
        // Interestingly, the dormant epoch counter is reset only once all certificates have been
        // processed, as a last step. This means that if a DRep registers in the same transaction
        // that a new proposal is submitted, it still get the bonus.
        // |
        // * TL; DR -> We need `<=` and not `<`.
        LastDormantEpoch::Last {
            ending_at: Some(ending_at),
            length,
        } => {
            if registered_at <= ending_at.slot {
                length
            } else {
                0
            }
        }
    }
}

#[derive(Debug)]
enum LastDormantEpoch<'a> {
    /// Starting case, indicating no active proposals whatsoever if returned.
    Empty { previous_epoch: Epoch },

    /// Similar to Empty, but contains additional information used while recursing.
    None {
        previous_epoch: (&'a TransactionPointer, &'a Epoch),
    },

    /// Actual result of a previous or ongoing dormant epoch.
    Last {
        ending_at: Option<&'a TransactionPointer>,
        length: u64,
    },
}

/// Search for the last dormant period in a series of proposals. Returns the certificate that
/// effectively puts an end to the dormant period, as well as the length of that period.
///
/// In case where the dormant period is still ongoing, returns `None` and the current length.
fn last_dormant_period(
    governance_action_lifetime: u64,
    registered_in: Epoch,
    proposals: BTreeSet<&(TransactionPointer, Epoch)>,
) -> LastDormantEpoch<'_> {
    let dormant_period = |previous_epoch, current_epoch| {
        if previous_epoch <= current_epoch + governance_action_lifetime {
            None
        } else {
            // We iterate on a BTreeSet in reverse order, so when in dormant periods, the previous
            // epoch is necessarily larger than the current one. And we've just guaranteed there's
            // a gap that is greater than the governance_action_lifetime between both.
            Some(previous_epoch - current_epoch - governance_action_lifetime)
        }
    };

    proposals.iter().rev().fold(
        LastDormantEpoch::Empty {
            previous_epoch: registered_in,
        },
        |st, current_epoch| {
            match st {
                // Short-circuit the fold if we've found any non-zero dormant epoch.
                LastDormantEpoch::Last { .. } => st,
                LastDormantEpoch::Empty { previous_epoch } => {
                    match dormant_period(previous_epoch, current_epoch.1) {
                        None => LastDormantEpoch::None {
                            previous_epoch: (&current_epoch.0, &current_epoch.1),
                        },
                        Some(length) => LastDormantEpoch::Last {
                            ending_at: None,
                            length,
                        },
                    }
                }
                LastDormantEpoch::None {
                    previous_epoch: (previous_pointer, previous_epoch),
                    ..
                } => match dormant_period(*previous_epoch, current_epoch.1) {
                    None => LastDormantEpoch::None {
                        previous_epoch: (&current_epoch.0, &current_epoch.1),
                    },
                    Some(length) => LastDormantEpoch::Last {
                        ending_at: Some(previous_pointer),
                        length,
                    },
                },
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use amaru_kernel::network::{Bound, EraParams, Summary};
    use std::sync::LazyLock;
    use test_case::test_case;
    use EpochResult::*;

    const VERSION_9: ProtocolVersion = (9, 0);
    const VERSION_10: ProtocolVersion = (10, 0);

    /// Some arbitrary, albeit simple, era history for testing purpose. Each epoch contains 10
    /// slots or 1s each. The era starts at 0 and ends after 100 epochs, so make sure tests are
    /// within this bound.
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

    /// Create a transaction pointer from a slot and a transaction index. Also returns the epoch
    /// corresponding to that slot.
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
        0, vec![]
        => None
    )]
    #[test_case(
        8, vec![ptr(85, 0)]
        => None
    )]
    #[test_case(
        15, vec![ptr(85, 0)]
        => Some((None, 4))
    )]
    #[test_case(
        12, vec![ptr(115, 0), ptr(85, 0), ptr(125, 0)]
        => None
    )]
    #[test_case(
        5, vec![ptr(15, 0), ptr(45, 0)]
        => None
    )]
    #[test_case(
        5, vec![ptr(15, 0), ptr(55, 0)]
        => Some((Some(ptr(55, 0).0), 1))
    )]
    #[test_case(
        6, vec![ptr(15, 0), ptr(65, 0)]
        => Some((Some(ptr(65, 0).0), 2))
    )]
    #[test_case(
        6, vec![ptr(15, 0), ptr(65, 0), ptr(65, 2)]
        => Some((Some(ptr(65, 0).0), 2))
    )]
    #[test_case(
        11, vec![ptr(15, 0), ptr(65, 0)]
        => Some((None, 2))
    )]
    #[test_case(
        14, vec![ptr(15, 0), ptr(65, 0), ptr(145, 0)]
        => Some((Some(ptr(145, 0).0), 5))
    )]
    fn test_last_dormant_period(
        current_epoch: Epoch,
        proposals: Vec<(TransactionPointer, Epoch)>,
    ) -> Option<(Option<TransactionPointer>, u64)> {
        match last_dormant_period(3, current_epoch, proposals.iter().collect()) {
            LastDormantEpoch::Empty { .. } | LastDormantEpoch::None { .. } => None,
            LastDormantEpoch::Last {
                ending_at, length, ..
            } => Some((ending_at.copied(), length)),
        }
    }

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
                current_epoch,
                proposals.clone(),
            )(
                (registration_slot, registration_epoch),
                last_interaction.unwrap_or(registration_epoch),
            )
        };

        let v9 = test_with(VERSION_9);

        let v10 = test_with(VERSION_10);

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
    #[test_case( 84, Some(9),  8 => Consistent(19))]
    #[test_case( 85,    None,  8 => Consistent(18))]
    #[test_case( 85, Some(9),  8 => Consistent(19))]
    #[test_case( 86,    None,  8 => Consistent(18))]
    #[test_case( 86, Some(9),  8 => Consistent(19))]
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
    #[test_case(115, Some(13), 12 => Consistent(23))]
    #[test_case(115, Some(13), 13 => Consistent(24))]
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
    #[test_case(140,    None, 11 => Consistent(24))]
    #[test_case(150,    None, 11 => Inconsistent{ v9: 26, v10: 25 })]
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
