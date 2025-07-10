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

use amaru_kernel::{EpochInterval, Slot, TransactionPointer};
use slot_arithmetic::Epoch;
use std::collections::BTreeSet;

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
pub(crate) fn drep_bonus_mandate(
    governance_action_lifetime: EpochInterval,
    proposals: &BTreeSet<(TransactionPointer, Epoch)>,
    (registered_at, registered_in): (Slot, Epoch),
) -> u64 {
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
pub enum LastDormantEpoch<'a> {
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
fn last_dormant_period<'a>(
    governance_action_lifetime: EpochInterval,
    registered_in: Epoch,
    proposals: BTreeSet<&'a (TransactionPointer, Epoch)>,
) -> LastDormantEpoch<'a> {
    let dormant_period = |previous_epoch, current_epoch| {
        if previous_epoch <= current_epoch + governance_action_lifetime as u64 {
            None
        } else {
            // We iterate on a BTreeSet in reverse order, so when in dormant periods, the previous
            // epoch is necessarily larger than the current one. And we've just guaranteed there's
            // a gap that is greater than the governance_action_lifetime between both.
            Some(previous_epoch - current_epoch - governance_action_lifetime as u64)
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
pub(crate) mod tests {
    use super::*;
    use amaru_kernel::{
        network::{Bound, EraParams, Summary},
        EraHistory,
    };
    use slot_arithmetic::Epoch;
    use std::sync::LazyLock;
    use test_case::test_case;

    /// Some arbitrary, albeit simple, era history for testing purpose. Each epoch contains 10
    /// slots or 1s each. The era starts at 0 and ends after 100 epochs, so make sure tests are
    /// within this bound.
    pub(crate) static ERA_HISTORY: LazyLock<EraHistory> = LazyLock::new(|| {
        EraHistory::new(
            &[Summary {
                start: Bound {
                    time_ms: 0,
                    slot: Slot::from(0),
                    epoch: Epoch::from(0),
                },
                end: Some(Bound {
                    time_ms: 1000000,
                    slot: Slot::from(1000),
                    epoch: Epoch::from(100),
                }),
                params: EraParams {
                    epoch_size_slots: 10,
                    slot_length: 1000,
                },
            }],
            Slot::from(10),
        )
    });

    /// Create a transaction pointer from a slot and a transaction index. Also returns the epoch
    /// corresponding to that slot.
    pub(crate) fn ptr(slot: u64, transaction_index: usize) -> (TransactionPointer, Epoch) {
        let slot = Slot::from(slot);
        (
            TransactionPointer {
                slot,
                transaction_index,
            },
            ERA_HISTORY.slot_to_epoch_unchecked_horizon(slot).unwrap(),
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
        current_epoch: u64,
        proposals: Vec<(TransactionPointer, Epoch)>,
    ) -> Option<(Option<TransactionPointer>, u64)> {
        match last_dormant_period(3, Epoch::from(current_epoch), proposals.iter().collect()) {
            LastDormantEpoch::Empty { .. } | LastDormantEpoch::None { .. } => None,
            LastDormantEpoch::Last {
                ending_at, length, ..
            } => Some((ending_at.copied(), length)),
        }
    }
}
