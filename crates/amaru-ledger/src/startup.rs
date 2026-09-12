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

use std::ops::Deref;

use amaru_kernel::{Epoch, EraHistory, GovernanceAction, ProtocolParameters};
use amaru_observability::{info, info_span};
use tracing::field;

use crate::store::{ReadStore, StoreError};

// ------------------------------------------------------------------------------------- StartupHook

pub type StartupHook<S> = fn(&StartupContext<'_, S>) -> Result<(), StoreError>;

pub fn no_startup_hook<S: ReadStore>(_: &StartupContext<'_, S>) -> Result<(), StoreError> {
    Ok(())
}

pub fn with_startup_hook<'a, S: ReadStore>(ctx: &StartupContext<'a, S>) -> Result<(), StoreError> {
    ctx.emit_protocol_parameters();
    ctx.emit_current_pots()?;
    ctx.emit_active_proposals()?;
    ctx.emit_constitutional_committee()?;
    Ok(())
}

// ---------------------------------------------------------------------------------- StartupContext

pub struct StartupContext<'a, S: ReadStore> {
    stable: &'a S,
    epoch: Epoch,
    protocol_parameters: &'a ProtocolParameters,
    era_history: &'a EraHistory,
}

impl<'a, S: ReadStore> Deref for StartupContext<'a, S> {
    type Target = S;
    fn deref(&self) -> &Self::Target {
        self.stable
    }
}

impl<'a, S: ReadStore> StartupContext<'a, S> {
    pub(crate) fn new(
        stable: &'a S,
        epoch: Epoch,
        protocol_parameters: &'a ProtocolParameters,
        era_history: &'a EraHistory,
    ) -> Self {
        Self { stable, epoch, protocol_parameters, era_history }
    }

    pub fn epoch(&self) -> Epoch {
        self.epoch
    }

    pub fn protocol_parameters(&self) -> &ProtocolParameters {
        self.protocol_parameters
    }

    pub fn era_history(&self) -> &EraHistory {
        self.era_history
    }

    fn emit_protocol_parameters(&self) {
        let ProtocolParameters {
            protocol_version,
            max_block_body_size,
            max_transaction_size,
            max_block_header_size,
            max_tx_ex_units,
            max_block_ex_units,
            max_value_size,
            max_collateral_inputs,
            min_fee_a,
            min_fee_b,
            stake_credential_deposit,
            stake_pool_deposit,
            monetary_expansion_rate,
            treasury_expansion_rate,
            min_pool_cost,
            lovelace_per_utxo_byte,
            prices,
            min_fee_ref_script_lovelace_per_byte,
            max_ref_script_size_per_tx,
            max_ref_script_size_per_block,
            ref_script_cost_stride,
            ref_script_cost_multiplier,
            stake_pool_max_retirement_epoch,
            optimal_stake_pools_count,
            pledge_influence,
            collateral_percentage,
            cost_models: _,
            pool_voting_thresholds,
            drep_voting_thresholds,
            min_committee_size,
            max_committee_term_length,
            gov_action_lifetime,
            gov_action_deposit,
            drep_deposit,
            drep_expiry,
        } = &self.protocol_parameters;

        info!(
            ledger::protocol_parameters::DUMP,
            protocol_version,
            max_block_body_size,
            max_transaction_size,
            max_block_header_size,
            max_tx_ex_units,
            max_block_ex_units,
            max_value_size,
            max_collateral_inputs,
            min_fee_a,
            min_fee_b,
            stake_credential_deposit,
            stake_pool_deposit,
            monetary_expansion_rate,
            treasury_expansion_rate,
            min_pool_cost,
            lovelace_per_utxo_byte,
            prices,
            min_fee_ref_script_lovelace_per_byte,
            max_ref_script_size_per_tx,
            max_ref_script_size_per_block,
            ref_script_cost_stride,
            ref_script_cost_multiplier,
            stake_pool_max_retirement_epoch,
            optimal_stake_pools_count,
            pledge_influence,
            collateral_percentage,
            pool_voting_thresholds,
            drep_voting_thresholds,
            min_committee_size,
            max_committee_term_length,
            gov_action_lifetime,
            gov_action_deposit,
            drep_deposit,
            drep_expiry,
        );
    }

    fn emit_current_pots(&self) -> Result<(), StoreError> {
        let pots = self.pots()?;

        info!(
            ledger::pots::DUMP,
            treasury = pots.treasury,
            reserves = pots.reserves,
            fees = pots.fees,
            donations = pots.donations,
        );

        Ok(())
    }

    fn emit_active_proposals(&self) -> Result<(), StoreError> {
        for (id, row) in self.iter_proposals()? {
            let proposal_kind = proposal_kind(&row.proposal.gov_action);
            let detail = proposal_detail(&row.proposal.gov_action);
            let proposed_in = self
                .era_history()
                .slot_to_epoch_unchecked_horizon(row.proposed_in.transaction.slot)
                .map_err(|error| StoreError::Internal(Box::new(error)))?;

            if let Some(detail) = detail {
                info!(
                    ledger::proposal::ACTIVE,
                    id = id.to_string(),
                    proposal_kind,
                    proposed_in,
                    valid_until = row.valid_until,
                    detail = @detail,
                );
            } else {
                info!(
                    ledger::proposal::ACTIVE,
                    id = id.to_string(),
                    proposal_kind,
                    proposed_in,
                    valid_until = row.valid_until,
                );
            }
        }

        Ok(())
    }

    fn emit_constitutional_committee(&self) -> Result<(), StoreError> {
        info_span!(ledger::constitutional_committee::DUMP, status = self.constitutional_committee()?);
        for (cold_credential, member) in self.iter_cc_members()? {
            info!(
                ledger::constitutional_committee_member::DUMP,
                cold_credential = cold_credential,
                status = @member.status.as_ref().map(field::display),
                valid_until = @member.valid_until.as_ref().map(|epoch| epoch.as_u64()),
            );
        }
        Ok(())
    }
}

// ----------------------------------------------------------------------------------------- Helpers

fn proposal_kind(proposal: &GovernanceAction) -> &'static str {
    match proposal {
        GovernanceAction::ParameterChange(..) => "protocol-parameters",
        GovernanceAction::HardForkInitiation(..) => "hard-fork",
        GovernanceAction::TreasuryWithdrawals(..) => "treasury-withdrawal",
        GovernanceAction::NoConfidence(..) => "motion-of-no-confidence",
        GovernanceAction::UpdateCommittee(..) => "constitutional-committee",
        GovernanceAction::NewConstitution(..) => "constitution",
        GovernanceAction::Information => "nice-poll",
    }
}

fn proposal_detail(proposal: &GovernanceAction) -> Option<String> {
    match proposal {
        GovernanceAction::HardForkInitiation(_, version) => Some(version.to_string()),
        GovernanceAction::TreasuryWithdrawals(withdrawals, _) => {
            Some(format!("{} lovelace", withdrawals.iter().map(|(_, amount)| *amount).sum::<u64>()))
        }
        GovernanceAction::UpdateCommittee(_, removed, added, threshold) => {
            Some(format!("removed={}, added={}, threshold={threshold}", removed.len(), added.len()))
        }
        GovernanceAction::NewConstitution(..) => Some("new constitution".to_string()),
        GovernanceAction::ParameterChange(..) => Some("protocol parameters".to_string()),
        GovernanceAction::NoConfidence(..) | GovernanceAction::Information => None,
    }
}
