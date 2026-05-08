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

use std::{collections::BTreeMap, str::FromStr, time::Duration};

use amaru_kernel::{
    CostModel, CostModels, DRepVotingThresholds, Epoch, EraBound, EraHistory, EraName, EraParams, EraSummary,
    ExUnitPrices, ExUnits, MemoizedTransactionOutput, NetworkName, PoolVotingThresholds, ProtocolParameters,
    ProtocolVersion, RationalNumber, Slot, TransactionInput, TransactionPointer, json,
    utils::serde::{deserialize_map_proxy, hex_to_bytes},
};
use serde::Deserialize;

use crate::{
    rules::transaction::phase_one::{InvalidVKeyWitness, PhaseOneError},
    store::GovernanceActivity,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Fixture {
    #[serde(deserialize_with = "deserialize_network_name")]
    pub(super) network: NetworkName,
    pub(super) era_history: EraHistoryFixture,
    pub(super) protocol_parameters: ProtocolParametersFixture,
    pub(super) initial_state: InitialState,
    pub(super) ledger_env: LedgerEnv,
    #[serde(deserialize_with = "hex_to_bytes")]
    pub(super) transaction: Vec<u8>,
    pub(super) expected: Expected,
}

#[derive(Debug)]
pub(super) enum Expected {
    Pass,
    Fail(Predicate),
}

impl<'de> Deserialize<'de> for Expected {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = json::Value::deserialize(d)?;
        match value {
            json::Value::String(s) if s == "Pass" => Ok(Expected::Pass),
            json::Value::Object(_) => json::from_value(value).map(Expected::Fail).map_err(serde::de::Error::custom),
            json::Value::String(s) => Err(serde::de::Error::custom(format!("expected \"Pass\", got {s:?}"))),
            json::Value::Null | json::Value::Bool(_) | json::Value::Number(_) | json::Value::Array(_) => {
                Err(serde::de::Error::custom("expected \"Pass\" or { predicate: ..., ... }"))
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "predicate")]
pub(super) enum Predicate {
    InvalidWitnessesUTXOW,
}

impl From<PhaseOneError> for Predicate {
    fn from(err: PhaseOneError) -> Self {
        match err {
            PhaseOneError::VKeyWitness(InvalidVKeyWitness::InvalidSignatures { .. }) => {
                Predicate::InvalidWitnessesUTXOW
            }
            PhaseOneError::VKeyWitness(_)
            | PhaseOneError::Inputs(_)
            | PhaseOneError::Outputs(_)
            | PhaseOneError::Certificates(_)
            | PhaseOneError::Fees(_)
            | PhaseOneError::Withdrawals(_)
            | PhaseOneError::Scripts(_)
            | PhaseOneError::Collateral(_)
            | PhaseOneError::Proposals(_)
            | PhaseOneError::Metadata(_)
            | PhaseOneError::InvalidNetworkID { .. }
            | PhaseOneError::TooLarge { .. } => unreachable!("no predicate mapping yet for {err}"),
        }
    }
}

fn deserialize_network_name<'de, D: serde::Deserializer<'de>>(d: D) -> Result<NetworkName, D::Error> {
    let s = String::deserialize(d)?;
    NetworkName::from_str(&s).map_err(serde::de::Error::custom)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct InitialState {
    #[serde(deserialize_with = "deserialize_map_proxy")]
    pub(super) utxo: BTreeMap<TransactionInput, MemoizedTransactionOutput>,
    pub(super) voting_state: VotingState,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct VotingState {
    num_dormant_epochs: u32,
}

impl From<VotingState> for GovernanceActivity {
    fn from(v: VotingState) -> Self {
        GovernanceActivity { consecutive_dormant_epochs: v.num_dormant_epochs }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LedgerEnv {
    slot: Slot,
    tx_ix: u64,
}

impl From<LedgerEnv> for TransactionPointer {
    fn from(env: LedgerEnv) -> Self {
        TransactionPointer { slot: env.slot, transaction_index: env.tx_ix.try_into().expect("tx_ix fits in usize") }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct EraHistoryFixture {
    stability_window: Slot,
    eras: Vec<EraSummaryFixture>,
}

impl From<EraHistoryFixture> for EraHistory {
    fn from(f: EraHistoryFixture) -> Self {
        let eras: Vec<EraSummary> = f.eras.into_iter().map(Into::into).collect();
        EraHistory::new(&eras, f.stability_window)
    }
}

#[derive(Debug, Deserialize)]
struct EraSummaryFixture {
    start: EraBoundFixture,
    end: Option<EraBoundFixture>,
    params: EraParamsFixture,
}

impl From<EraSummaryFixture> for EraSummary {
    fn from(f: EraSummaryFixture) -> Self {
        EraSummary { start: f.start.into(), end: f.end.map(Into::into), params: f.params.into() }
    }
}

#[derive(Debug, Deserialize)]
struct EraBoundFixture {
    time: u64,
    slot: u64,
    epoch: u64,
}

impl From<EraBoundFixture> for EraBound {
    fn from(f: EraBoundFixture) -> Self {
        EraBound { time: Duration::from_secs(f.time), slot: Slot::new(f.slot), epoch: Epoch::new(f.epoch) }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EraParamsFixture {
    epoch_size_slots: u64,
    slot_length_ms: u64,
    era_name: EraName,
}

impl From<EraParamsFixture> for EraParams {
    fn from(f: EraParamsFixture) -> Self {
        EraParams {
            epoch_size_slots: f.epoch_size_slots,
            slot_length: Duration::from_millis(f.slot_length_ms),
            era_name: f.era_name,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ProtocolVersionFixture {
    major: u64,
    minor: u64,
}

impl From<ProtocolVersionFixture> for ProtocolVersion {
    fn from(f: ProtocolVersionFixture) -> Self {
        (f.major, f.minor)
    }
}

#[derive(Debug, Deserialize)]
struct RationalFixture {
    numerator: u64,
    denominator: u64,
}

impl From<RationalFixture> for RationalNumber {
    fn from(f: RationalFixture) -> Self {
        RationalNumber { numerator: f.numerator, denominator: f.denominator }
    }
}

#[derive(Debug, Deserialize)]
struct ExecutionUnitsFixture {
    memory: u64,
    cpu: u64,
}

impl From<ExecutionUnitsFixture> for ExUnits {
    fn from(f: ExecutionUnitsFixture) -> Self {
        ExUnits { mem: f.memory, steps: f.cpu }
    }
}

#[derive(Debug, Deserialize)]
struct ScriptExecutionPricesFixture {
    memory: RationalFixture,
    cpu: RationalFixture,
}

impl From<ScriptExecutionPricesFixture> for ExUnitPrices {
    fn from(f: ScriptExecutionPricesFixture) -> Self {
        ExUnitPrices { mem_price: f.memory.into(), step_price: f.cpu.into() }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConstitutionalCommitteeThresholds {
    default: RationalFixture,
    state_of_no_confidence: RationalFixture,
}

#[derive(Debug, Deserialize)]
struct StakePoolPpuThresholds {
    security: RationalFixture,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StakePoolVotingThresholdsFixture {
    no_confidence: RationalFixture,
    constitutional_committee: ConstitutionalCommitteeThresholds,
    hard_fork_initiation: RationalFixture,
    protocol_parameters_update: StakePoolPpuThresholds,
}

impl From<StakePoolVotingThresholdsFixture> for PoolVotingThresholds {
    fn from(f: StakePoolVotingThresholdsFixture) -> Self {
        PoolVotingThresholds {
            motion_no_confidence: f.no_confidence.into(),
            committee_normal: f.constitutional_committee.default.into(),
            committee_no_confidence: f.constitutional_committee.state_of_no_confidence.into(),
            hard_fork_initiation: f.hard_fork_initiation.into(),
            security_voting_threshold: f.protocol_parameters_update.security.into(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct DRepPpuThresholds {
    network: RationalFixture,
    economic: RationalFixture,
    technical: RationalFixture,
    governance: RationalFixture,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DRepVotingThresholdsFixture {
    no_confidence: RationalFixture,
    constitution: RationalFixture,
    constitutional_committee: ConstitutionalCommitteeThresholds,
    hard_fork_initiation: RationalFixture,
    protocol_parameters_update: DRepPpuThresholds,
    treasury_withdrawals: RationalFixture,
}

impl From<DRepVotingThresholdsFixture> for DRepVotingThresholds {
    fn from(f: DRepVotingThresholdsFixture) -> Self {
        DRepVotingThresholds {
            motion_no_confidence: f.no_confidence.into(),
            committee_normal: f.constitutional_committee.default.into(),
            committee_no_confidence: f.constitutional_committee.state_of_no_confidence.into(),
            update_constitution: f.constitution.into(),
            hard_fork_initiation: f.hard_fork_initiation.into(),
            pp_network_group: f.protocol_parameters_update.network.into(),
            pp_economic_group: f.protocol_parameters_update.economic.into(),
            pp_technical_group: f.protocol_parameters_update.technical.into(),
            pp_governance_group: f.protocol_parameters_update.governance.into(),
            treasury_withdrawal: f.treasury_withdrawals.into(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct PlutusCostModelsFixture {
    #[serde(rename = "plutusV1")]
    plutus_v1: Option<CostModel>,
    #[serde(rename = "plutusV2")]
    plutus_v2: Option<CostModel>,
    #[serde(rename = "plutusV3")]
    plutus_v3: Option<CostModel>,
}

impl From<PlutusCostModelsFixture> for CostModels {
    fn from(f: PlutusCostModelsFixture) -> Self {
        CostModels { plutus_v1: f.plutus_v1, plutus_v2: f.plutus_v2, plutus_v3: f.plutus_v3 }
    }
}

#[derive(Debug, Deserialize)]
struct MinFeeReferenceScripts {
    range: u32,
    base: RationalFixture,
    multiplier: RationalFixture,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProtocolParametersFixture {
    min_fee_coefficient: u64,
    min_fee_constant: u64,
    min_fee_reference_scripts: MinFeeReferenceScripts,
    min_utxo_deposit_coefficient: u64,
    max_block_body_size: u64,
    max_block_header_size: u16,
    max_transaction_size: u64,
    max_value_size: u64,
    max_reference_scripts_size: u32,
    stake_credential_deposit: u64,
    stake_pool_deposit: u64,
    stake_pool_retirement_epoch_bound: u64,
    stake_pool_pledge_influence: RationalFixture,
    min_stake_pool_cost: u64,
    desired_number_of_stake_pools: u16,
    monetary_expansion: RationalFixture,
    treasury_expansion: RationalFixture,
    collateral_percentage: u16,
    max_collateral_inputs: u16,
    plutus_cost_models: PlutusCostModelsFixture,
    script_execution_prices: ScriptExecutionPricesFixture,
    max_execution_units_per_transaction: ExecutionUnitsFixture,
    max_execution_units_per_block: ExecutionUnitsFixture,
    stake_pool_voting_thresholds: StakePoolVotingThresholdsFixture,
    constitutional_committee_min_size: u16,
    constitutional_committee_max_term_length: u64,
    governance_action_lifetime: u64,
    governance_action_deposit: u64,
    delegate_representative_voting_thresholds: DRepVotingThresholdsFixture,
    delegate_representative_deposit: u64,
    delegate_representative_max_idle_time: u64,
    version: ProtocolVersionFixture,
}

impl From<ProtocolParametersFixture> for ProtocolParameters {
    fn from(f: ProtocolParametersFixture) -> Self {
        ProtocolParameters {
            protocol_version: f.version.into(),
            min_fee_a: f.min_fee_coefficient,
            min_fee_b: f.min_fee_constant,
            max_block_body_size: f.max_block_body_size,
            max_transaction_size: f.max_transaction_size,
            max_block_header_size: f.max_block_header_size,
            stake_credential_deposit: f.stake_credential_deposit,
            stake_pool_deposit: f.stake_pool_deposit,
            stake_pool_max_retirement_epoch: f.stake_pool_retirement_epoch_bound,
            optimal_stake_pools_count: f.desired_number_of_stake_pools,
            pledge_influence: f.stake_pool_pledge_influence.into(),
            monetary_expansion_rate: f.monetary_expansion.into(),
            treasury_expansion_rate: f.treasury_expansion.into(),
            min_pool_cost: f.min_stake_pool_cost,
            lovelace_per_utxo_byte: f.min_utxo_deposit_coefficient,
            prices: f.script_execution_prices.into(),
            max_tx_ex_units: f.max_execution_units_per_transaction.into(),
            max_block_ex_units: f.max_execution_units_per_block.into(),
            max_value_size: f.max_value_size,
            collateral_percentage: f.collateral_percentage,
            max_collateral_inputs: f.max_collateral_inputs,
            pool_voting_thresholds: f.stake_pool_voting_thresholds.into(),
            drep_voting_thresholds: f.delegate_representative_voting_thresholds.into(),
            min_committee_size: f.constitutional_committee_min_size,
            max_committee_term_length: f.constitutional_committee_max_term_length,
            gov_action_lifetime: f.governance_action_lifetime,
            gov_action_deposit: f.governance_action_deposit,
            drep_deposit: f.delegate_representative_deposit,
            drep_expiry: f.delegate_representative_max_idle_time,
            min_fee_ref_script_lovelace_per_byte: f.min_fee_reference_scripts.base.into(),
            cost_models: f.plutus_cost_models.into(),
            max_ref_script_size_per_tx: f.max_reference_scripts_size,
            // Ogmios exposes a single max-ref-scripts size; the per-block bound is hardcoded in the Haskell ledger.
            max_ref_script_size_per_block: 1024 * 1024,
            ref_script_cost_stride: f.min_fee_reference_scripts.range,
            ref_script_cost_multiplier: f.min_fee_reference_scripts.multiplier.into(),
        }
    }
}
