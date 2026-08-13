// Copyright 2026 PRAGMA
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
    ops::Neg,
};

use amaru_kernel::{
    AsHash, ConstitutionalCommitteeStatus, ConstitutionalCommitteeUpdate, Hash, Lovelace, PoolId, ProposalId,
    ProtocolParameters, RatificationStatus, RationalNumber, StakeCredential, StakeCredentialKind, size::SCRIPT,
};
use amaru_observability::{debug, debug_span};
use num::BigUint;
use tracing::Span;

use crate::{
    epoch_transition::{Effective, GovernanceActivity, GovernanceUpdates, Rewards},
    store::{StoreError, TransactionalContext, columns::pools::Row as Pool},
};

// -------------------------------------------------------------------------------------------------
// ------------------------------------------------------------------------------------ End of epoch
// -------------------------------------------------------------------------------------------------

/// Pay rewards to all accounts before the epoch ends.
pub fn pay_rewards<'store>(
    db: &impl TransactionalContext<'store>,
    effective_rewards: &Rewards<Effective>,
) -> Result<(), StoreError> {
    debug_span!(stores::ledger::overlay::PAY_REWARDS).in_scope(|| {

        // Pay rewards out to every account
        db.with_accounts(|iterator| {
            let mut rewards_paid: u64 = 0;
            let mut accounts_paid: u64 = 0;

            for (credential, mut row) in iterator {
                let rewards = effective_rewards.reward_of(&credential);

                if rewards > 0 {
                    // The condition avoids the mutable borrow when not needed,
                    // which will incur a db operation.
                    if let Some(account) = row.borrow_mut() {
                        accounts_paid += 1;
                        rewards_paid += rewards;
                        account.rewards += rewards;
                    }
                }
            }

            let expected_total_rewards = effective_rewards.total_rewards();
            let actual_total_rewards = rewards_paid + effective_rewards.total_unclaimed_rewards();

            // Technically, if we did everything *right*, there should be no accounts with rewards that
            // cannot be paid out (i.e. accounts that no longer exists). This has been taken care of during
            // the epoch transition calculations already. So at this point, this invariant must hold: every
            // payable reward account was found while iterating the stored accounts.
            assert!(
                actual_total_rewards == expected_total_rewards,
                "discrepancy between expected total rewards (={expected_total_rewards}) and actual total rewards (={actual_total_rewards})",
            );

            Span::current().record("accounts_paid", accounts_paid);
            Span::current().record("rewards_paid", rewards_paid);
        })?;

        // Adjust treasury and reserves accordingly.
        db.with_pots(|mut row| {
            let pots = row.borrow_mut();

            let delta_treasury = effective_rewards.delta_treasury();
            pots.treasury += delta_treasury;
            Span::current().record("treasury_delta", delta_treasury);

            let delta_reserves = effective_rewards.delta_reserves();
            pots.reserves -= delta_reserves;
            Span::current().record("reserves_delta", (delta_reserves as i64).neg());
        })?;

        Ok(())
    })
}

/// Retain recently pruned proposals (ratified, expired or dropped due to other ratifying to
/// expiring) for the epoch, to allow resolving voting stake distribution for the epoch correctly.
pub fn reset_recently_pruned_proposals<'store>(
    db: &impl TransactionalContext<'store>,
    pruned_proposals: BTreeMap<&ProposalId, RatificationStatus>,
) -> Result<(), StoreError> {
    debug_span!(stores::ledger::overlay::RECORD_PRUNED_PROPOSALS).in_scope(|| {
        db.set_recently_pruned_proposals(pruned_proposals)?;
        Ok(())
    })
}

// -------------------------------------------------------------------------------------------------
// ---------------------------------------------------------------------------------- Start of epoch
// -------------------------------------------------------------------------------------------------

pub fn reset_fees_and_donations<'store>(db: &impl TransactionalContext<'store>) -> Result<(), StoreError> {
    debug_span!(stores::ledger::overlay::RESET_FEES).in_scope(|| {
        db.with_pots(|mut row| {
            let row = row.borrow_mut();
            row.fees = 0;
            row.treasury += row.donations;
            row.donations = 0;
        })
    })
}

pub fn reset_blocks_count<'store>(db: &impl TransactionalContext<'store>) -> Result<(), StoreError> {
    debug_span!(stores::ledger::overlay::RESET_BLOCKS_COUNT,).in_scope(|| {
        // TODO: Dropping entire RocksDB columns
        //
        // If necessary, come up with a more efficient way of dropping a "table".
        // RocksDB does support batch-removing of key ranges, but somehow, not in a
        // transactional way. So it isn't as trivial to implement as it may seem.
        db.with_block_issuers(|iterator| {
            for (_, mut row) in iterator {
                *row.borrow_mut() = None;
            }
        })
    })
}

/// Return deposits back to reward accounts, adding leftovers to the treasury.
pub fn pay_or_refund_accounts<'store, 'iter>(
    db: &impl TransactionalContext<'store>,
    payouts: impl IntoIterator<Item = (&'iter StakeCredential, &'iter Lovelace)>,
) -> Result<(), StoreError> {
    debug_span!(stores::ledger::overlay::PAY_OR_REFUND_ACCOUNTS,).in_scope(|| {
        let (leftovers, paid) = payouts.into_iter().try_fold::<_, _, Result<_, StoreError>>(
            (0_u64, 0_u64),
            |(leftovers, paid), (account, deposit)| {
                debug!(
                    ledger::account::PAY_OR_REFUND,
                    credential_type = %StakeCredentialKind::from(account),
                    account = %account.as_hash(),
                    %deposit,
                );

                Ok((leftovers + db.refund(account, *deposit)?, paid + *deposit))
            },
        )?;

        Span::current().record("total_paid_or_refunded", paid - leftovers);
        Span::current().record("treasury_leftovers", leftovers);

        if leftovers > 0 {
            db.with_pots(|mut pots| pots.borrow_mut().treasury += leftovers)?;
        }

        Ok(())
    })
}

/// Update pool parameters now valid at an epoch boundary, and retire pools that have reached their
/// retirement epoch.
pub fn update_or_retire_pools<'store>(
    db: &impl TransactionalContext<'store>,
    updates: &BTreeMap<PoolId, Pool>,
    retirements: &BTreeSet<PoolId>,
) -> Result<(), StoreError> {
    debug_span!(
        stores::ledger::overlay::UPDATE_OR_RETIRE_POOLS,
        pools_updated = updates.len() as u64,
        pools_retired = retirements.len() as u64,
    )
    .in_scope(|| {
        // TODO: multi-modify without full iterations?
        //
        // This quite inefficient, as we have to iterate through ALL pools just to possibly update a
        // few. It is reasonable to assume that the number of updates is vastly smaller to the total
        // number of pools. I don't feel like modifying the store handle to do that now, though...
        //
        // Given that the total number of pools is limited anyway; this is "acceptable".
        db.with_pools(|iterator| {
            // Note that we don't trace anything here since traces already happen in the
            // epoch_transition::pools_update module; when those updates are first computed.
            //
            // We only clone the handful of pool rows that actually changed this epoch, so the flush
            // can borrow the (potentially recovery-shared) updates rather than consuming them.
            for (id, mut row) in iterator {
                if retirements.contains(&id) {
                    *row.borrow_mut() = None;
                } else if let Some(pool) = updates.get(&id) {
                    *row.borrow_mut() = Some(pool.clone())
                }
            }
        })
    })
}

/// Flush the result of a governance ratification to disk., This includes:
///
/// - New proposal roots for the protocol parameters, constitution, constitutional committee and
///   hard forks proposal types.
///
/// - A new/modified constitution
///
/// - Addition, removal or change of threshold in the constitutional committee
///
/// - Withdrawals to accounts (and deposits refunds)
///
/// - Change to protocol parameters
///
/// Note that this also removes proposals that are now either enacted, expired or simply pruned to
/// a parent also being pruned.
pub fn apply_governance_updates<'store>(
    db: &impl TransactionalContext<'store>,
    updates: &GovernanceUpdates,
) -> Result<(ProtocolParameters, GovernanceActivity, Option<Hash<SCRIPT>>), StoreError> {
    debug_span!(stores::ledger::overlay::APPLY_GOVERNANCE_UPDATES,).in_scope(|| {
        db.set_proposals_roots(&updates.roots)?;

        let guardrail_script = match updates.new_constitution.as_ref() {
            Some(new_constitution) => {
                db.set_constitution(new_constitution)?;
                new_constitution.guardrail_script
            }
            None => db.constitution()?.guardrail_script,
        };

        if let Some(committee_update) = updates.constitutional_committee.as_ref() {
            update_constitutional_committee(db, committee_update)?;
        }

        db.set_protocol_parameters(&updates.protocol_parameters)?;

        pay_or_refund_accounts(db, updates.deposit_refunds.iter().chain(updates.treasury_withdrawals.iter()))?;

        // Withdrawals move money out of the treasury into reward accounts. Deposit refunds do
        // not; they were never part of the treasury. Withdrawals whose target account no longer
        // exists flow back into the treasury as leftovers just above.
        if !updates.treasury_withdrawals.is_empty() {
            db.with_pots(|mut row| {
                let pots = row.borrow_mut();
                pots.treasury -= updates.treasury_withdrawals.values().sum::<Lovelace>();
            })?;
        }

        let mut governance_activity = db.governance_activity()?;
        if updates.is_dormant_epoch {
            governance_activity.consecutive_dormant_epochs += 1;
            debug!(
                ledger::governance_activity::UPDATE,
                consecutive_dormant_epochs = governance_activity.consecutive_dormant_epochs
            );
            db.set_governance_activity(governance_activity)?;
        }

        db.remove_proposals(updates.pruned_proposals.keys())?;

        Ok((updates.protocol_parameters.clone(), governance_activity, guardrail_script))
    })
}

/// Flush updates to the constitutional committee.
pub fn update_constitutional_committee<'store>(
    db: &impl TransactionalContext<'store>,
    committee_update: &ConstitutionalCommitteeUpdate,
) -> Result<(), StoreError> {
    debug_span!(
        stores::ledger::overlay::UPDATE_CONSTITUTIONAL_COMMITTEE,
        no_confidence = matches!(committee_update, ConstitutionalCommitteeUpdate::NoConfidence)
    )
    .in_scope(|| {
        match committee_update {
            ConstitutionalCommitteeUpdate::NoConfidence => {
                db.update_constitutional_committee(
                    &ConstitutionalCommitteeStatus::NoConfidence,
                    BTreeMap::new(),
                    BTreeSet::new(),
                )?;

                db.with_cc_members(|iterator| {
                    // NOTE: CC members and no-confidence mode
                    //
                    // CC members are not deleted when entering no confidence
                    // mode. They are simply marked as inactive.
                    //
                    // In particular, their hot<->cold bindings are preserved.
                    for (_, mut row) in iterator {
                        if let Some(cc_member) = row.borrow_mut() {
                            cc_member.valid_until = None;
                        }
                    }
                })
            }

            ConstitutionalCommitteeUpdate::ChangeMembers { removed, added, threshold } => {
                let unsafe_u64 = |lbl: &str, n: &BigUint| {
                    n.try_into().unwrap_or_else(|e| unreachable!("threshold {lbl}={n} larger than u64?!: {e}"))
                };

                let committee_status = ConstitutionalCommitteeStatus::Trusted {
                    threshold: RationalNumber {
                        numerator: unsafe_u64("numerator", threshold.numer()),
                        denominator: unsafe_u64("denominator", threshold.denom()),
                    },
                };

                // The committee is a handful of members, so cloning the added/removed sets to hand
                // owned values to the store is negligible and lets the caller borrow the update.
                db.update_constitutional_committee(&committee_status, added.clone(), removed.clone())
            }
        }
    })
}
