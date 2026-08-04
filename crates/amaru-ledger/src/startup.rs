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

use amaru_kernel::{Epoch, EraHistory, GovernanceAction, ProtocolParameters, protocol_version};
use amaru_observability::info;

use crate::store::{self, ReadStore, StoreError};

pub type StartupHook<S> = fn(&Database<'_, S>) -> Result<(), StoreError>;

pub struct Database<'a, S: ReadStore> {
    stable: &'a S,
    epoch: Epoch,
    protocol_parameters: &'a ProtocolParameters,
    era_history: &'a EraHistory,
}

impl<'a, S: ReadStore> Database<'a, S> {
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

    pub fn pots(&self) -> Result<store::columns::pots::Row, StoreError> {
        self.stable.pots()
    }

    pub fn iter_proposals(
        &self,
    ) -> Result<impl Iterator<Item = (store::columns::proposals::Key, store::columns::proposals::Row)> + '_, StoreError>
    {
        self.stable.iter_proposals()
    }
}

pub fn with_startup_hook<S: ReadStore>(database: &Database<'_, S>) -> Result<(), StoreError> {
    emit_protocol_parameters(database);
    emit_current_pots(database)?;
    emit_active_proposals(database)?;
    Ok(())
}

pub fn no_startup_hook<S: ReadStore>(_: &Database<'_, S>) -> Result<(), StoreError> {
    Ok(())
}

fn emit_protocol_parameters<S: ReadStore>(database: &Database<'_, S>) {
    let protocol_parameters = database.protocol_parameters();

    info!(
        ledger::protocol_parameters::LOAD,
        protocol_version = protocol_version::fmt(&protocol_parameters.protocol_version),
        max_block_body_size = protocol_parameters.max_block_body_size.to_string(),
        max_transaction_size = protocol_parameters.max_transaction_size.to_string(),
        max_tx_ex_units = protocol_parameters.max_tx_ex_units.to_string(),
        max_block_ex_units = protocol_parameters.max_block_ex_units.to_string(),
        min_fee_a = protocol_parameters.min_fee_a.to_string(),
        min_fee_b = protocol_parameters.min_fee_b.to_string(),
        stake_credential_deposit = protocol_parameters.stake_credential_deposit.to_string(),
        stake_pool_deposit = protocol_parameters.stake_pool_deposit.to_string(),
        lovelace_per_utxo_byte = protocol_parameters.lovelace_per_utxo_byte.to_string(),
        collateral_percentage = protocol_parameters.collateral_percentage.to_string(),
        gov_action_lifetime = protocol_parameters.gov_action_lifetime.to_string(),
        gov_action_deposit = protocol_parameters.gov_action_deposit.to_string(),
        drep_deposit = protocol_parameters.drep_deposit.to_string(),
        drep_expiry = protocol_parameters.drep_expiry.to_string(),
    );
}

fn emit_current_pots<S: ReadStore>(database: &Database<'_, S>) -> Result<(), StoreError> {
    let pots = database.pots()?;

    info!(
        ledger::pots::LOAD,
        treasury = pots.treasury,
        reserves = pots.reserves,
        fees = pots.fees,
        donations = pots.donations,
    );

    Ok(())
}

fn emit_active_proposals<S: ReadStore>(database: &Database<'_, S>) -> Result<(), StoreError> {
    for (id, row) in database.iter_proposals()? {
        let proposal_kind = proposal_kind(&row.proposal.gov_action);
        let detail = proposal_detail(&row.proposal.gov_action);
        let proposed_in = database
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
                detail,
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
        GovernanceAction::HardForkInitiation(_, version) => Some(protocol_version::fmt(version)),
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
