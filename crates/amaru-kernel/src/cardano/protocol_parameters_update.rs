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
    CostModels, DRepVotingThresholds, ExUnitPrices, ExUnits, Lovelace, PoolVotingThresholds, RationalNumber,
    UnitRationalNumber, cbor,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, cbor::Encode)]
#[cbor(context_bound = "crate::cbor::HasProtocolVersion")]
#[cbor(map)]
pub struct ProtocolParamUpdate {
    #[n(0)]
    pub minfee_a: Option<u64>,
    #[n(1)]
    pub minfee_b: Option<u64>,
    #[n(2)]
    pub max_block_body_size: Option<u32>,
    #[n(3)]
    pub max_transaction_size: Option<u32>,
    #[n(4)]
    pub max_block_header_size: Option<u16>,
    #[n(5)]
    pub key_deposit: Option<Lovelace>,
    #[n(6)]
    pub pool_deposit: Option<Lovelace>,
    #[n(7)]
    pub maximum_epoch: Option<u32>,
    #[n(8)]
    pub desired_number_of_stake_pools: Option<u16>,
    #[n(9)]
    pub pool_pledge_influence: Option<RationalNumber>,
    #[n(10)]
    pub expansion_rate: Option<UnitRationalNumber>,
    #[n(11)]
    pub treasury_growth_rate: Option<UnitRationalNumber>,
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
    pub max_value_size: Option<u32>,
    #[n(23)]
    pub collateral_percentage: Option<u16>,
    #[n(24)]
    pub max_collateral_inputs: Option<u16>,
    #[n(25)]
    pub pool_voting_thresholds: Option<PoolVotingThresholds>,
    #[n(26)]
    pub drep_voting_thresholds: Option<DRepVotingThresholds>,
    #[n(27)]
    pub min_committee_size: Option<u16>,
    #[n(28)]
    pub committee_term_limit: Option<u32>,
    #[n(29)]
    pub governance_action_validity_period: Option<u32>,
    #[n(30)]
    pub governance_action_deposit: Option<Lovelace>,
    #[n(31)]
    pub drep_deposit: Option<Lovelace>,
    #[n(32)]
    pub drep_inactivity_period: Option<u32>,
    #[n(33)]
    pub minfee_refscript_cost_per_byte: Option<RationalNumber>,
}

impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for ProtocolParamUpdate {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_map_unique_keys(
            d,
            ProtocolParamUpdate::default(),
            |d| d.u64(),
            |d, st, k| {
                match k {
                    0 => st.minfee_a = Some(d.decode_with(ctx)?),
                    1 => st.minfee_b = Some(d.decode_with(ctx)?),
                    2 => st.max_block_body_size = Some(d.decode_with(ctx)?),
                    3 => st.max_transaction_size = Some(d.decode_with(ctx)?),
                    4 => st.max_block_header_size = Some(d.decode_with(ctx)?),
                    5 => st.key_deposit = Some(d.decode_with(ctx)?),
                    6 => st.pool_deposit = Some(d.decode_with(ctx)?),
                    7 => st.maximum_epoch = Some(d.decode_with(ctx)?),
                    8 => st.desired_number_of_stake_pools = Some(d.decode_with(ctx)?),
                    9 => st.pool_pledge_influence = Some(d.decode_with(ctx)?),
                    10 => st.expansion_rate = Some(d.decode_with(ctx)?),
                    11 => st.treasury_growth_rate = Some(d.decode_with(ctx)?),
                    16 => st.min_pool_cost = Some(d.decode_with(ctx)?),
                    17 => st.ada_per_utxo_byte = Some(d.decode_with(ctx)?),
                    18 => st.cost_models_for_script_languages = Some(d.decode_with(ctx)?),
                    19 => st.execution_costs = Some(d.decode_with(ctx)?),
                    20 => st.max_tx_ex_units = Some(d.decode_with(ctx)?),
                    21 => st.max_block_ex_units = Some(d.decode_with(ctx)?),
                    22 => st.max_value_size = Some(d.decode_with(ctx)?),
                    23 => st.collateral_percentage = Some(d.decode_with(ctx)?),
                    24 => st.max_collateral_inputs = Some(d.decode_with(ctx)?),
                    25 => st.pool_voting_thresholds = Some(d.decode_with(ctx)?),
                    26 => st.drep_voting_thresholds = Some(d.decode_with(ctx)?),
                    27 => st.min_committee_size = Some(d.decode_with(ctx)?),
                    28 => st.committee_term_limit = Some(d.decode_with(ctx)?),
                    29 => st.governance_action_validity_period = Some(d.decode_with(ctx)?),
                    30 => st.governance_action_deposit = Some(d.decode_with(ctx)?),
                    31 => st.drep_deposit = Some(d.decode_with(ctx)?),
                    32 => st.drep_inactivity_period = Some(d.decode_with(ctx)?),
                    33 => st.minfee_refscript_cost_per_byte = Some(d.decode_with(ctx)?),
                    _ => {
                        let position = d.position();
                        return Err(cbor::decode::Error::message(format!("unrecognised field key: {k}")).at(position));
                    }
                };

                Ok(())
            },
        )
    }
}

impl ProtocolParamUpdate {
    // Check whether the update contains any parameter that is considered part of the 'security group'.
    // Those parameters require approval from the SPO to be changed. Others are only in the hands of
    // DReps & Constitutional Committee.
    pub fn any_in_security_group(&self) -> bool {
        self.minfee_a.is_some()
            || self.minfee_b.is_some()
            || self.max_block_body_size.is_some()
            || self.max_block_header_size.is_some()
            || self.max_transaction_size.is_some()
            || self.ada_per_utxo_byte.is_some()
            || self.max_block_ex_units.is_some()
            || self.max_value_size.is_some()
            || self.governance_action_deposit.is_some()
            || self.minfee_refscript_cost_per_byte.is_some()
    }
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
            cn = v.committee_normal,
            cc = v.committee_no_confidence,
            mnc = v.motion_no_confidence,
            hfi = v.hard_fork_initiation,
            svt = v.security_voting_threshold,
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
            cn = v.committee_normal,
            cc = v.committee_no_confidence,
            mnc = v.motion_no_confidence,
            tw = v.treasury_withdrawal,
            uc = v.update_constitution,
            ppn = v.pp_network_group,
            ppe = v.pp_economic_group,
            ppt = v.pp_technical_group,
            ppg = v.pp_governance_group,
            hfi = v.hard_fork_initiation,
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

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;
    use crate::from_cbor_no_leftovers;

    /// The ledger reads this map with `decodeSparseKeyed`, which fails on a key it does not know
    /// and on a key it has already seen. Keys 12 to 15 were never assigned, so they are unknown.
    #[test_case("a0"                  => matches Ok(_)  ; "no update at all")]
    #[test_case("a1000a"              => matches Ok(_)  ; "one known key")]
    #[test_case("a2000a010b"          => matches Ok(_)  ; "two distinct known keys")]
    #[test_case("a2000a000b"          => matches Err(_) ; "the same key twice")]
    #[test_case("a10c0a"              => matches Err(_) ; "key 12, never assigned")]
    #[test_case("a118630a"            => matches Err(_) ; "a key beyond the last one")]
    #[test_case("a2000a18630a"        => matches Err(_) ; "a known key and an unknown one")]
    fn decode_rejects_unknown_and_duplicate_keys(input: &str) -> Result<ProtocolParamUpdate, cbor::decode::Error> {
        from_cbor_no_leftovers(&hex::decode(input).unwrap())
    }
}
