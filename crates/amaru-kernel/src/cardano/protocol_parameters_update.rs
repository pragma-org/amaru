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

use std::fmt::{self, Write};

use crate::{
    CostModels, DRepVotingThresholds, ExUnitPrices, ExUnits, Lovelace, PoolVotingThresholds, RationalNumber, cbor,
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, cbor::Encode, cbor::Decode)]
#[cbor(map)]
pub struct ProtocolParamUpdate {
    #[n(0)]
    pub minfee_a: Option<u64>,
    #[n(1)]
    pub minfee_b: Option<u64>,
    #[n(2)]
    pub max_block_body_size: Option<u64>,
    #[n(3)]
    pub max_transaction_size: Option<u64>,
    #[n(4)]
    pub max_block_header_size: Option<u64>,
    #[n(5)]
    pub key_deposit: Option<Lovelace>,
    #[n(6)]
    pub pool_deposit: Option<Lovelace>,
    #[n(7)]
    pub maximum_epoch: Option<u64>,
    #[n(8)]
    pub desired_number_of_stake_pools: Option<u64>,
    #[n(9)]
    pub pool_pledge_influence: Option<RationalNumber>,
    #[n(10)]
    pub expansion_rate: Option<RationalNumber>,
    #[n(11)]
    pub treasury_growth_rate: Option<RationalNumber>,
    #[n(16)]
    pub min_pool_cost: Option<Lovelace>,
    #[n(17)]
    pub ada_per_utxo_byte: Option<Lovelace>,
    #[n(18)]
    pub cost_models_for_script_languages: Option<CostModels>,
    #[n(19)]
    pub execution_costs: Option<ExUnitPrices>,
    #[n(20)]
    pub max_tx_ex_units: Option<ExUnits>,
    #[n(21)]
    pub max_block_ex_units: Option<ExUnits>,
    #[n(22)]
    pub max_value_size: Option<u64>,
    #[n(23)]
    pub collateral_percentage: Option<u64>,
    #[n(24)]
    pub max_collateral_inputs: Option<u64>,
    #[n(25)]
    pub pool_voting_thresholds: Option<PoolVotingThresholds>,
    #[n(26)]
    pub drep_voting_thresholds: Option<DRepVotingThresholds>,
    #[n(27)]
    pub min_committee_size: Option<u64>,
    #[n(28)]
    pub committee_term_limit: Option<u64>,
    #[n(29)]
    pub governance_action_validity_period: Option<u64>,
    #[n(30)]
    pub governance_action_deposit: Option<Lovelace>,
    #[n(31)]
    pub drep_deposit: Option<Lovelace>,
    #[n(32)]
    pub drep_inactivity_period: Option<u64>,
    #[n(33)]
    pub minfee_refscript_cost_per_byte: Option<RationalNumber>,
}

pub fn display_protocol_parameters_update(update: &ProtocolParamUpdate, prefix: &str) -> Result<String, fmt::Error> {
    let mut s = String::new();

    fn push_opt<T: fmt::Display>(
        out: &mut String,
        is_first: &mut bool,
        prefix: &str,
        name: &str,
        v: &Option<T>,
    ) -> fmt::Result {
        if let Some(x) = v {
            if *is_first {
                *is_first = false;
            } else {
                writeln!(out)?;
            }
            write!(out, "{prefix}{name}={x}")?;
        }
        Ok(())
    }

    let mut is_first = true;

    push_opt(&mut s, &mut is_first, prefix, "minfee_a", &update.minfee_a)?;

    push_opt(&mut s, &mut is_first, prefix, "minfee_b", &update.minfee_b)?;

    push_opt(&mut s, &mut is_first, prefix, "max_block_body_size", &update.max_block_body_size)?;

    push_opt(&mut s, &mut is_first, prefix, "max_transaction_size", &update.max_transaction_size)?;

    push_opt(&mut s, &mut is_first, prefix, "max_block_header_size", &update.max_block_header_size)?;

    push_opt(&mut s, &mut is_first, prefix, "key_deposit", &update.key_deposit)?;

    push_opt(&mut s, &mut is_first, prefix, "pool_deposit", &update.pool_deposit)?;

    push_opt(&mut s, &mut is_first, prefix, "maximum_epoch", &update.maximum_epoch)?;

    push_opt(&mut s, &mut is_first, prefix, "desired_number_of_stake_pools", &update.desired_number_of_stake_pools)?;

    push_opt(&mut s, &mut is_first, prefix, "pool_pledge_influence", &update.pool_pledge_influence)?;

    push_opt(&mut s, &mut is_first, prefix, "expansion_rate", &update.expansion_rate)?;

    push_opt(&mut s, &mut is_first, prefix, "treasury_growth_rate", &update.treasury_growth_rate)?;

    push_opt(&mut s, &mut is_first, prefix, "min_pool_cost", &update.min_pool_cost)?;

    push_opt(&mut s, &mut is_first, prefix, "lovelace_per_utxo_byte", &update.ada_per_utxo_byte)?;

    // If you don’t want to expand cost models, just mark them as set.
    let cost_models = update.cost_models_for_script_languages.as_ref().map(|cost_models| {
        let mut languages = vec![];
        if cost_models.plutus_v1.is_some() {
            languages.push("v1");
        }
        if cost_models.plutus_v2.is_some() {
            languages.push("v2");
        }
        if cost_models.plutus_v3.is_some() {
            languages.push("v3");
        }
        languages.join(", ")
    });
    push_opt(&mut s, &mut is_first, prefix, "cost_models", &cost_models)?;

    push_opt(&mut s, &mut is_first, prefix, "execution_costs", &update.execution_costs)?;

    push_opt(&mut s, &mut is_first, prefix, "max_tx_ex_units", &update.max_tx_ex_units)?;

    push_opt(&mut s, &mut is_first, prefix, "max_block_ex_units", &update.max_block_ex_units)?;

    push_opt(&mut s, &mut is_first, prefix, "max_value_size", &update.max_value_size)?;

    push_opt(&mut s, &mut is_first, prefix, "collateral_percentage", &update.collateral_percentage)?;

    push_opt(&mut s, &mut is_first, prefix, "max_collateral_inputs", &update.max_collateral_inputs)?;

    let pool_voting = update.pool_voting_thresholds.as_ref().map(|v| {
        format!(
            "\n{p}  ├─ committee (normal)         {cn}\
             \n{p}  ├─ committee (no confidence)  {cc}\
             \n{p}  ├─ motion of no confidence    {mnc}\
             \n{p}  ├─ hard fork                  {hfi}\
             \n{p}  └─ protocol params (security) {svt}",
            p = prefix,
            cn = &v.committee_normal,
            cc = &v.committee_no_confidence,
            mnc = &v.motion_no_confidence,
            hfi = &v.hard_fork_initiation,
            svt = &v.security_voting_threshold,
        )
    });
    push_opt(&mut s, &mut is_first, prefix, "pool_voting_thresholds", &pool_voting)?;

    let drep_voting = update.drep_voting_thresholds.as_ref().map(|v| {
        format!(
            "\n{p}  ├─ committee (normal)           {cn}\
             \n{p}  ├─ committee (no confidence)    {cc}\
             \n{p}  ├─ motion of no confidence      {mnc}\
             \n{p}  ├─ treasury withdrawal          {tw}\
             \n{p}  ├─ constitution                 {uc}\
             \n{p}  ├─ protocol params (network)    {ppn}\
             \n{p}  ├─ protocol params (economic)   {ppe}\
             \n{p}  ├─ protocol params (technical)  {ppt}\
             \n{p}  ├─ protocol params (governance) {ppg}\
             \n{p}  └─ hard fork                    {hfi}",
            p = prefix,
            cn = &v.committee_normal,
            cc = &v.committee_no_confidence,
            mnc = &v.motion_no_confidence,
            tw = &v.treasury_withdrawal,
            uc = &v.update_constitution,
            ppn = &v.pp_network_group,
            ppe = &v.pp_economic_group,
            ppt = &v.pp_technical_group,
            ppg = &v.pp_governance_group,
            hfi = &v.hard_fork_initiation,
        )
    });

    push_opt(&mut s, &mut is_first, prefix, "drep_voting_thresholds", &drep_voting)?;

    push_opt(&mut s, &mut is_first, prefix, "min_committee_size", &update.min_committee_size)?;

    push_opt(&mut s, &mut is_first, prefix, "committee_term_limit", &update.committee_term_limit)?;

    push_opt(
        &mut s,
        &mut is_first,
        prefix,
        "governance_action_validity_period",
        &update.governance_action_validity_period,
    )?;

    push_opt(&mut s, &mut is_first, prefix, "governance_action_deposit", &update.governance_action_deposit)?;

    push_opt(&mut s, &mut is_first, prefix, "drep_deposit", &update.drep_deposit)?;

    push_opt(&mut s, &mut is_first, prefix, "drep_inactivity_period", &update.drep_inactivity_period)?;

    push_opt(&mut s, &mut is_first, prefix, "minfee_refscript_cost_per_byte", &update.minfee_refscript_cost_per_byte)?;

    Ok(s)
}
