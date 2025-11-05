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

pub use crate::{
    CostModel, CostModels, DRepVotingThresholds, ExUnitPrices, ExUnits, PoolVotingThresholds,
    ProtocolParamUpdate,
};
use crate::{
    EpochInterval, Language, Lovelace, PoolId, ProtocolVersion, RationalNumber, cbor,
    heterogeneous_array,
};
use amaru_slot_arithmetic::{EraHistory, Slot};
pub use default::*;
use pallas_codec::minicbor::{Decoder, data::Tag};
use pallas_math::math::{FixedDecimal, FixedPrecision};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;

mod default;

/// Model from https://github.com/IntersectMBO/formal-ledger-specifications/blob/master/src/Ledger/PParams.lagda
/// Some of the names have been adapted to improve readability.
/// Also see https://github.com/IntersectMBO/cardano-ledger/blob/d90eb4df4651970972d860e95f1a3697a3de8977/eras/conway/impl/cddl-files/conway.cddl#L324
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolParameters {
    // Outside of all groups.
    pub protocol_version: ProtocolVersion,

    // Network group
    pub max_block_body_size: u64,
    pub max_transaction_size: u64,
    pub max_block_header_size: u16,
    pub max_tx_ex_units: ExUnits,
    pub max_block_ex_units: ExUnits,
    pub max_value_size: u64,
    pub max_collateral_inputs: u16,

    // Economic group
    pub min_fee_a: Lovelace,
    pub min_fee_b: u64,
    pub stake_credential_deposit: Lovelace,
    pub stake_pool_deposit: Lovelace,
    pub monetary_expansion_rate: RationalNumber,
    pub treasury_expansion_rate: RationalNumber,
    pub min_pool_cost: u64,
    pub lovelace_per_utxo_byte: Lovelace,
    pub prices: ExUnitPrices,
    pub min_fee_ref_script_lovelace_per_byte: RationalNumber,
    pub max_ref_script_size_per_tx: u32,
    pub max_ref_script_size_per_block: u32,
    pub ref_script_cost_stride: u32,
    pub ref_script_cost_multiplier: RationalNumber,

    // Technical group
    pub stake_pool_max_retirement_epoch: EpochInterval,
    pub optimal_stake_pools_count: u16,
    pub pledge_influence: RationalNumber,
    pub collateral_percentage: u16,
    pub cost_models: Box<CostModels>,

    // Governance group
    pub pool_voting_thresholds: Box<PoolVotingThresholds>,
    pub drep_voting_thresholds: Box<DRepVotingThresholds>,
    pub min_committee_size: u16,
    pub max_committee_term_length: EpochInterval,
    pub gov_action_lifetime: EpochInterval,
    pub gov_action_deposit: Lovelace,
    pub drep_deposit: Lovelace,
    pub drep_expiry: EpochInterval,
}

impl ProtocolParameters {
    pub fn update(&mut self, u: ProtocolParamUpdate) {
        #[inline]
        fn set<T>(field: &mut T, opt: Option<T>) {
            if let Some(val) = opt {
                *field = val
            }
        }
        set(&mut self.min_fee_a, u.minfee_a);
        set(&mut self.min_fee_b, u.minfee_b);
        set(&mut self.max_block_body_size, u.max_block_body_size);
        set(&mut self.max_transaction_size, u.max_transaction_size);
        set(
            &mut self.max_block_header_size,
            // FIXME: update in Pallas; should be a u16
            u.max_block_header_size.map(|x| x as u16),
        );
        set(&mut self.stake_credential_deposit, u.key_deposit);
        set(&mut self.stake_pool_deposit, u.pool_deposit);
        set(&mut self.stake_pool_max_retirement_epoch, u.maximum_epoch);
        set(
            &mut self.optimal_stake_pools_count,
            // FIXME: update in Pallas; should be a u16
            u.desired_number_of_stake_pools.map(|x| x as u16),
        );
        set(&mut self.pledge_influence, u.pool_pledge_influence);
        set(&mut self.treasury_expansion_rate, u.expansion_rate);
        set(&mut self.monetary_expansion_rate, u.treasury_growth_rate);
        set(&mut self.min_pool_cost, u.min_pool_cost);
        set(&mut self.lovelace_per_utxo_byte, u.ada_per_utxo_byte);
        if let Some(cost_models) = u.cost_models_for_script_languages {
            // NOTE: This code may looks a little convoluted here, but it exists for the sake of
            // generating a compiler error in due time. Should we not do that, and add a new language,
            // it is highly likely that we may forget to apply the corresponding cost model update for
            // that language.
            //
            // Now, we'll get the following pattern-match to fail due to non exhaustivness.
            match Language::PlutusV1 {
                Language::PlutusV1 => {
                    if let Some(plutus_v1) = cost_models.plutus_v1 {
                        self.cost_models.plutus_v1 = Some(plutus_v1);
                    }
                }
                Language::PlutusV2 | Language::PlutusV3 => (),
            }
            if let Some(plutus_v2) = cost_models.plutus_v2 {
                self.cost_models.plutus_v2 = Some(plutus_v2);
            }
            if let Some(plutus_v3) = cost_models.plutus_v3 {
                self.cost_models.plutus_v3 = Some(plutus_v3);
            }
        }
        set(&mut self.prices, u.execution_costs);
        set(&mut self.max_tx_ex_units, u.max_tx_ex_units);
        set(&mut self.max_block_ex_units, u.max_block_ex_units);
        set(&mut self.max_value_size, u.max_value_size);
        set(
            &mut self.collateral_percentage,
            // FIXME: update in Pallas; should be a u16
            u.collateral_percentage.map(|x| x as u16),
        );
        set(
            &mut self.max_collateral_inputs,
            // FIXME: update in Pallas; should be a u16
            u.max_collateral_inputs.map(|x| x as u16),
        );
        set(
            &mut self.pool_voting_thresholds,
            u.pool_voting_thresholds.map(Box::new),
        );
        set(
            &mut self.drep_voting_thresholds,
            u.drep_voting_thresholds.map(Box::new),
        );
        set(
            &mut self.min_committee_size,
            // FIXME: update in Pallas; should be a u16
            u.min_committee_size.map(|x| x as u16),
        );
        set(&mut self.max_committee_term_length, u.committee_term_limit);
        set(
            &mut self.gov_action_lifetime,
            u.governance_action_validity_period,
        );
        set(&mut self.gov_action_deposit, u.governance_action_deposit);
        set(&mut self.drep_deposit, u.drep_deposit);
        set(&mut self.drep_expiry, u.drep_inactivity_period);
        set(
            &mut self.min_fee_ref_script_lovelace_per_byte,
            u.minfee_refscript_cost_per_byte,
        );
    }
}

fn allow_tag(d: &mut Decoder<'_>, expected: Tag) -> Result<(), cbor::decode::Error> {
    if d.datatype()? == cbor::data::Type::Tag {
        let tag = d.tag()?;
        if tag != expected {
            return Err(cbor::decode::Error::message(format!(
                "invalid CBOR tag: expected {expected} got {tag}"
            )));
        }
    }

    Ok(())
}

fn decode_rationale(d: &mut Decoder<'_>) -> Result<RationalNumber, cbor::decode::Error> {
    allow_tag(d, Tag::new(30))?;
    heterogeneous_array(d, |d, assert_len| {
        assert_len(2)?;
        let numerator = d.u64()?;
        let denominator = d.u64()?;
        Ok(RationalNumber {
            numerator,
            denominator,
        })
    })
}

fn decode_protocol_version(d: &mut Decoder<'_>) -> Result<ProtocolVersion, cbor::decode::Error> {
    heterogeneous_array(d, |d, assert_len| {
        assert_len(2)?;
        let major = d.u8()?;

        // See: https://github.com/IntersectMBO/cardano-ledger/blob/693218df6cd90263da24e6c2118bac420ceea3a1/eras/conway/impl/cddl-files/conway.cddl#L126
        if major > 12 {
            return Err(cbor::decode::Error::message(
                "invalid protocol version's major: too high",
            ));
        }
        Ok((major as u64, d.u64()?))
    })
}

impl<'b, C> cbor::decode::Decode<'b, C> for ProtocolParameters {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        let min_fee_a = d.u64()?;
        let min_fee_b = d.u64()?;
        let max_block_body_size = d.u64()?;
        let max_transaction_size = d.u64()?;
        let max_block_header_size = d.u16()?;
        let stake_credential_deposit = d.u64()?;
        let stake_pool_deposit = d.u64()?;
        let stake_pool_max_retirement_epoch = d.u64()?;
        let optimal_stake_pools_count = d.u16()?;
        let pledge_influence = decode_rationale(d)?;
        let monetary_expansion_rate = decode_rationale(d)?;
        let treasury_expansion_rate = decode_rationale(d)?;
        let protocol_version = decode_protocol_version(d)?;
        let min_pool_cost = d.u64()?;
        let lovelace_per_utxo_byte = d.u64()?;

        let mut plutus_v1 = None;
        let mut plutus_v2 = None;
        let mut plutus_v3 = None;
        let i = d.map_iter_with::<C, u8, CostModel>(ctx)?;
        for item in i {
            let (k, v) = item?;
            match k {
                0 => {
                    plutus_v1 = Some(v);
                }
                1 => {
                    plutus_v2 = Some(v);
                }
                2 => {
                    plutus_v3 = Some(v);
                }
                _ => unreachable!("unexpected language version: {k}"),
            }
        }
        let prices = d.decode_with(ctx)?;
        let max_tx_ex_units = d.decode_with(ctx)?;
        let max_block_ex_units = d.decode_with(ctx)?;
        let max_value_size = d.u64()?;
        let collateral_percentage = d.u16()?;
        let max_collateral_inputs = d.u16()?;
        let pool_voting_thresholds = d.decode_with(ctx)?;
        let drep_voting_thresholds = d.decode_with(ctx)?;
        let min_committee_size = d.u16()?;
        let max_committee_term_length = d.u64()?;
        let gov_action_lifetime = d.u64()?;
        let gov_action_deposit = d.u64()?;
        let drep_deposit = d.u64()?;
        let drep_expiry = d.decode_with(ctx)?;
        let min_fee_ref_script_lovelace_per_byte = decode_rationale(d)?;

        Ok(ProtocolParameters {
            protocol_version,
            min_fee_a,
            min_fee_b,
            max_block_body_size,
            max_transaction_size,
            max_block_header_size,
            stake_credential_deposit,
            stake_pool_deposit,
            stake_pool_max_retirement_epoch,
            optimal_stake_pools_count,
            pledge_influence,
            monetary_expansion_rate,
            treasury_expansion_rate,
            min_pool_cost,
            lovelace_per_utxo_byte,
            cost_models: Box::new(CostModels {
                plutus_v1,
                plutus_v2,
                plutus_v3,
            }),
            prices,
            max_tx_ex_units,
            max_block_ex_units,
            max_value_size,
            collateral_percentage,
            max_collateral_inputs,
            pool_voting_thresholds,
            drep_voting_thresholds,
            min_committee_size,
            max_committee_term_length,
            gov_action_lifetime,
            gov_action_deposit,
            drep_deposit,
            drep_expiry,
            min_fee_ref_script_lovelace_per_byte,
            max_ref_script_size_per_tx: 200 * 1024, //Hardcoded in the haskell ledger (https://github.com/IntersectMBO/cardano-ledger/blob/3fe73a26588876bbf033bf4c4d25c97c2d8564dd/eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Ledger.hs#L154)
            max_ref_script_size_per_block: 1024 * 1024, // Hardcoded in the haskell ledger (https://github.com/IntersectMBO/cardano-ledger/blob/3fe73a26588876bbf033bf4c4d25c97c2d8564dd/eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Bbody.hs#L91)
            ref_script_cost_stride: 25600, // Hardcoded in the haskell ledger (https://github.com/IntersectMBO/cardano-ledger/blob/3fe73a26588876bbf033bf4c4d25c97c2d8564dd/eras/conway/impl/src/Cardano/Ledger/Conway/Tx.hs#L82)
            ref_script_cost_multiplier: RationalNumber {
                numerator: 12,
                denominator: 10,
            }, // Hardcoded in the haskell ledger (https://github.com/IntersectMBO/cardano-ledger/blob/3fe73a26588876bbf033bf4c4d25c97c2d8564dd/eras/conway/impl/src/Cardano/Ledger/Conway/Tx.hs#L85)
        })
    }
}

fn encode_rationale<W: cbor::encode::Write>(
    e: &mut cbor::Encoder<W>,
    rat: &RationalNumber,
) -> Result<(), cbor::encode::Error<W::Error>> {
    e.tag(Tag::new(30))?;
    e.array(2)?;

    e.u64(rat.numerator)?;
    e.u64(rat.denominator)?;
    Ok(())
}

fn encode_protocol_version<W: cbor::encode::Write>(
    e: &mut cbor::Encoder<W>,
    v: &ProtocolVersion,
) -> Result<(), cbor::encode::Error<W::Error>> {
    e.array(2)?;
    e.u64(v.0)?;
    e.u64(v.1)?;
    Ok(())
}

impl<C> cbor::encode::Encode<C> for ProtocolParameters {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(31)?;
        e.u64(self.min_fee_a)?;
        e.u64(self.min_fee_b)?;
        e.u64(self.max_block_body_size)?;
        e.u64(self.max_transaction_size)?;
        e.u16(self.max_block_header_size)?;
        e.u64(self.stake_credential_deposit)?;
        e.u64(self.stake_pool_deposit)?;
        e.u64(self.stake_pool_max_retirement_epoch)?;
        e.u16(self.optimal_stake_pools_count)?;
        encode_rationale(e, &self.pledge_influence)?;
        encode_rationale(e, &self.monetary_expansion_rate)?;
        encode_rationale(e, &self.treasury_expansion_rate)?;
        encode_protocol_version(e, &self.protocol_version)?;
        e.u64(self.min_pool_cost)?;
        e.u64(self.lovelace_per_utxo_byte)?;

        let mut count = 0;
        if self.cost_models.plutus_v1.is_some() {
            count += 1;
        }
        if self.cost_models.plutus_v2.is_some() {
            count += 1;
        }
        if self.cost_models.plutus_v3.is_some() {
            count += 1;
        }
        e.map(count)?;
        if let Some(v) = self.cost_models.plutus_v1.as_ref() {
            e.u8(0)?;
            e.encode_with(v, ctx)?;
        }
        if let Some(v) = self.cost_models.plutus_v2.as_ref() {
            e.u8(1)?;
            e.encode_with(v, ctx)?;
        }
        if let Some(v) = self.cost_models.plutus_v3.as_ref() {
            e.u8(2)?;
            e.encode_with(v, ctx)?;
        }

        e.encode_with(&self.prices, ctx)?;
        e.encode_with(self.max_tx_ex_units, ctx)?;
        e.encode_with(self.max_block_ex_units, ctx)?;

        e.u64(self.max_value_size)?;
        e.u16(self.collateral_percentage)?;
        e.u16(self.max_collateral_inputs)?;

        e.encode_with(&self.pool_voting_thresholds, ctx)?;
        e.encode_with(&self.drep_voting_thresholds, ctx)?;

        e.u16(self.min_committee_size)?;
        e.u64(self.max_committee_term_length)?;
        e.u64(self.gov_action_lifetime)?;
        e.u64(self.gov_action_deposit)?;
        e.u64(self.drep_deposit)?;
        e.encode_with(self.drep_expiry, ctx)?;
        encode_rationale(e, &self.min_fee_ref_script_lovelace_per_byte)?;

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GlobalParameters {
    /// The maximum depth of a rollback, also known as the security parameter 'k'.
    /// This translates down to the length of our volatile storage, containing states of the ledger
    /// which aren't yet considered final.
    pub consensus_security_param: usize,

    /// Multiplier applied to the CONSENSUS_SECURITY_PARAM to determine the epoch length.
    pub epoch_length_scale_factor: usize,

    /// Inverse of the active slot coefficient (i.e. 1/f);
    pub active_slot_coeff_inverse: usize,

    /// Maximum supply of Ada, in lovelace (1 Ada = 1,000,000 Lovelace)
    pub max_lovelace_supply: Lovelace,

    /// Number of slots for a single KES validity period.
    pub slots_per_kes_period: u64,

    /// Maximum number of KES key evolution. Combined with SLOTS_PER_KES_PERIOD, these values
    /// indicates the validity period of a KES key before a new one is required.
    pub max_kes_evolution: u8,

    /// Number of slots in an epoch
    pub epoch_length: usize,

    /// Relative slot from which data of the previous epoch can be considered stable.
    pub stability_window: Slot,

    /// Number of slots at the end of each epoch which do NOT contribute randomness to the candidate
    /// nonce of the following epoch.
    pub randomness_stabilization_window: u64,

    /// POSIX time (milliseconds) of the System Start.
    pub system_start: u64,
}

/// This data type encapsulates the parameters needed by the consensus layer to operate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConsensusParameters {
    randomness_stabilization_window: u64,
    slots_per_kes_period: u64,
    max_kes_evolution: u64,
    active_slot_coeff: SerializedFixedDecimal,
    era_history: EraHistory,
    ocert_counters: BTreeMap<PoolId, u64>,
}

#[derive(Clone, Debug, PartialEq)]
struct SerializedFixedDecimal(FixedDecimal);

impl Serialize for SerializedFixedDecimal {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'a> Deserialize<'a> for SerializedFixedDecimal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'a>,
    {
        let s = String::deserialize(deserializer)?;
        FixedDecimal::from_str(&s, s.len() as u64)
            .map(SerializedFixedDecimal)
            .map_err(serde::de::Error::custom)
    }
}

impl ConsensusParameters {
    /// Create new consensus parameters from the given global parameters.
    pub fn new(
        global_parameters: GlobalParameters,
        era_history: &EraHistory,
        ocert_counters: BTreeMap<PoolId, u64>,
    ) -> Self {
        Self::create(
            global_parameters.randomness_stabilization_window,
            global_parameters.slots_per_kes_period,
            global_parameters.max_kes_evolution as u64,
            1f64 / global_parameters.active_slot_coeff_inverse as f64,
            era_history,
            ocert_counters,
        )
    }

    /// Create new consensus parameters from individual values.
    pub fn create(
        randomness_stabilization_window: u64,
        slots_per_kes_period: u64,
        max_kes_evolution: u64,
        active_slot_coeff: f64,
        era_history: &EraHistory,
        ocert_counters: BTreeMap<PoolId, u64>,
    ) -> ConsensusParameters {
        let active_slot_coeff =
            FixedDecimal::from((active_slot_coeff * 100.0) as u64) / FixedDecimal::from(100u64);
        Self {
            randomness_stabilization_window,
            slots_per_kes_period,
            max_kes_evolution,
            active_slot_coeff: SerializedFixedDecimal(active_slot_coeff),
            era_history: era_history.clone(),
            ocert_counters,
        }
    }

    pub fn era_history(&self) -> &EraHistory {
        &self.era_history
    }

    pub fn randomness_stabilization_window(&self) -> u64 {
        self.randomness_stabilization_window
    }

    pub fn slot_to_kes_period(&self, slot: Slot) -> u64 {
        u64::from(slot) / self.slots_per_kes_period
    }

    pub fn max_kes_evolutions(&self) -> u64 {
        self.max_kes_evolution
    }

    pub fn latest_opcert_sequence_number(&self, pool_id: &PoolId) -> Option<u64> {
        self.ocert_counters.get(pool_id).copied()
    }

    pub fn active_slot_coeff(&self) -> FixedDecimal {
        self.active_slot_coeff.0.clone()
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use super::PREPROD_INITIAL_PROTOCOL_PARAMETERS;
    use crate::{
        CostModel, DRepVotingThresholds, ExUnitPrices, ExUnits, GovAction, KeyValuePairs, Lovelace,
        Nullable, PoolVotingThresholds, ProposalId, ProtocolParamUpdate, RewardAccount, ScriptHash,
        Set, StakeCredential,
        protocol_parameters::{CostModels, ProtocolParameters, ProtocolVersion},
        tests::{
            any_constitution, any_nullable, any_proposal_id, any_rational_number,
            any_reward_account, any_script_hash, any_stake_credential,
        },
    };
    use proptest::{collection, option, prelude::*};

    #[cfg(not(target_os = "windows"))]
    use crate::prop_cbor_roundtrip;

    #[cfg(not(target_os = "windows"))]
    prop_cbor_roundtrip!(ProtocolParameters, any_protocol_parameter());

    prop_compose! {
        pub fn any_ex_units()(
            mem in any::<u64>(),
            steps in any::<u64>(),
        ) -> ExUnits {
            ExUnits {
                mem,
                steps,
            }
        }
    }

    prop_compose! {
        pub fn any_ex_units_prices()(
            mem_price in any_rational_number(),
            step_price in any_rational_number(),
        ) -> ExUnitPrices {
            ExUnitPrices {
                mem_price,
                step_price,
            }
        }
    }

    prop_compose! {
        pub fn any_protocol_version()(
            major in any::<u8>(),
            minor in any::<u64>(),
        ) -> ProtocolVersion {
            ((major % 13) as u64, minor)
        }
    }

    prop_compose! {
        pub fn any_drep_voting_thresholds()(
            motion_no_confidence in any_rational_number(),
            committee_normal in any_rational_number(),
            committee_no_confidence in any_rational_number(),
            update_constitution in any_rational_number(),
            hard_fork_initiation in any_rational_number(),
            pp_network_group in any_rational_number(),
            pp_economic_group in any_rational_number(),
            pp_technical_group in any_rational_number(),
            pp_governance_group in any_rational_number(),
            treasury_withdrawal in any_rational_number(),
        ) -> DRepVotingThresholds {
            DRepVotingThresholds {
                motion_no_confidence,
                committee_normal,
                committee_no_confidence,
                update_constitution,
                hard_fork_initiation,
                pp_network_group,
                pp_economic_group,
                pp_technical_group,
                pp_governance_group,
                treasury_withdrawal,
            }
        }
    }

    prop_compose! {
        pub fn any_pool_voting_thresholds()(
            motion_no_confidence in any_rational_number(),
            committee_normal in any_rational_number(),
            committee_no_confidence in any_rational_number(),
            hard_fork_initiation in any_rational_number(),
            security_voting_threshold in any_rational_number(),
        ) -> PoolVotingThresholds {
            PoolVotingThresholds {
                motion_no_confidence,
                committee_normal,
                committee_no_confidence,
                hard_fork_initiation,
                security_voting_threshold,
            }
        }
    }

    prop_compose! {
        pub fn any_cost_model()(
            machine_cost in option::of(any::<i64>()),
            some_builtin in option::of(any::<i64>()),
            some_other_builtin in option::of(any::<i64>()),
        ) -> CostModel {
            vec![
                machine_cost,
                some_builtin,
                some_other_builtin,
            ]
            .into_iter()
            .flatten()
            .collect()
        }
    }

    prop_compose! {
        pub fn any_cost_models()(
            plutus_v1 in option::of(any_cost_model()),
            plutus_v2 in option::of(any_cost_model()),
            plutus_v3 in option::of(any_cost_model()),
        ) -> CostModels {
            CostModels {
                plutus_v1,
                plutus_v2,
                plutus_v3,
            }
        }
    }

    prop_compose! {
        pub fn any_ex_unit_prices()(
            mem_price in any_rational_number(),
            step_price in any_rational_number(),
        ) -> ExUnitPrices {
            ExUnitPrices {
                mem_price,
                step_price,
            }
        }
    }

    prop_compose! {
        pub fn any_protocol_params_update()(
            minfee_a in option::of(any::<u64>()),
            minfee_b in option::of(any::<u64>()),
            max_block_body_size in option::of(any::<u64>()),
            max_transaction_size in option::of(any::<u64>()),
            max_block_header_size in option::of(any::<u64>()),
            key_deposit in option::of(any::<Lovelace>()),
            pool_deposit in option::of(any::<Lovelace>()),
            maximum_epoch in option::of(any::<u64>()),
            desired_number_of_stake_pools in option::of(any::<u64>()),
            pool_pledge_influence in option::of(any_rational_number()),
            expansion_rate in option::of(any_rational_number()),
            treasury_growth_rate in option::of(any_rational_number()),
            min_pool_cost in option::of(any::<Lovelace>()),
            ada_per_utxo_byte in option::of(any::<Lovelace>()),
            cost_models_for_script_languages in option::of(any_cost_models()),
            execution_costs in option::of(any_ex_unit_prices()),
            max_tx_ex_units in option::of(any_ex_units()),
            max_block_ex_units in option::of(any_ex_units()),
            max_value_size in option::of(any::<u64>()),
            collateral_percentage in option::of(any::<u64>()),
            max_collateral_inputs in option::of(any::<u64>()),
            pool_voting_thresholds in option::of(any_pool_voting_thresholds()),
            drep_voting_thresholds in option::of(any_drep_voting_thresholds()),
            min_committee_size in option::of(any::<u64>()),
            committee_term_limit in option::of(any::<u64>()),
            governance_action_validity_period in option::of(any::<u64>()),
            governance_action_deposit in option::of(any::<Lovelace>()),
            drep_deposit in option::of(any::<Lovelace>()),
            drep_inactivity_period in option::of(any::<u64>()),
            minfee_refscript_cost_per_byte in option::of(any_rational_number()),
        ) -> ProtocolParamUpdate {
            ProtocolParamUpdate {
                minfee_a,
                minfee_b,
                max_block_body_size,
                max_transaction_size,
                max_block_header_size,
                key_deposit,
                pool_deposit,
                maximum_epoch,
                desired_number_of_stake_pools,
                pool_pledge_influence,
                expansion_rate,
                treasury_growth_rate,
                min_pool_cost,
                ada_per_utxo_byte,
                cost_models_for_script_languages,
                execution_costs,
                max_tx_ex_units,
                max_block_ex_units,
                max_value_size,
                collateral_percentage,
                max_collateral_inputs,
                pool_voting_thresholds,
                drep_voting_thresholds,
                min_committee_size,
                committee_term_limit,
                governance_action_validity_period,
                governance_action_deposit,
                drep_deposit,
                drep_inactivity_period,
                minfee_refscript_cost_per_byte,
            }
        }
    }

    pub fn any_gov_action() -> impl Strategy<Value = GovAction> {
        prop_compose! {
            fn any_parent_proposal_id()(
                proposal_id in option::of(any_proposal_id()),
            ) -> Nullable<ProposalId> {
                Nullable::from(proposal_id)
            }
        }

        prop_compose! {
            fn any_action_parameter_change()(
                parent_proposal_id in any_parent_proposal_id(),
                pparams in any_protocol_params_update(),
                guardrails in any_guardrails_script(),
            ) -> GovAction {
                GovAction::ParameterChange(parent_proposal_id, Box::new(pparams), guardrails)
            }
        }

        prop_compose! {
            fn any_hardfork_initiation()(
                parent_proposal_id in any_parent_proposal_id(),
                protocol_version in any_protocol_version(),
            ) -> GovAction {
                GovAction::HardForkInitiation(parent_proposal_id, protocol_version)
            }
        }

        prop_compose! {
            fn any_treasury_withdrawals()(
                withdrawals in collection::vec(any_withdrawal(), 0..3),
                guardrails in any_guardrails_script(),
                is_definite in any::<bool>(),
            ) -> GovAction {
                GovAction::TreasuryWithdrawals(
                    if is_definite {
                        KeyValuePairs::Def(withdrawals)
                    } else {
                        KeyValuePairs::Indef(withdrawals)
                    },
                    guardrails
                )
            }
        }

        prop_compose! {
            fn any_no_confidence()(
                parent_proposal_id in any_parent_proposal_id(),
            ) -> GovAction {
                GovAction::NoConfidence(parent_proposal_id)
            }
        }

        prop_compose! {
            fn any_committee_registration()(
                credential in any_stake_credential(),
                epoch in any::<u64>(),
            ) -> (StakeCredential, u64) {
                (credential, epoch)
            }
        }

        prop_compose! {
            fn any_committee_update()(
                parent_proposal_id in any_parent_proposal_id(),
                to_remove in collection::btree_set(any_stake_credential(), 0..3),
                to_add in collection::vec(any_committee_registration(), 0..3),
                is_definite in any::<bool>(),
                quorum in any_rational_number(),
            ) -> GovAction {
                GovAction::UpdateCommittee(
                    parent_proposal_id,
                    Set::from(to_remove.into_iter().collect::<Vec<_>>()),
                    if is_definite {
                        KeyValuePairs::Def(to_add)
                    } else {
                        KeyValuePairs::Indef(to_add)
                    },
                    quorum
                )
            }
        }

        prop_compose! {
            fn any_new_constitution()(
                parent_proposal_id in any_parent_proposal_id(),
                constitution in any_constitution(),
            ) -> GovAction {
                GovAction::NewConstitution(parent_proposal_id, constitution)
            }
        }

        fn any_nice_poll() -> impl Strategy<Value = GovAction> {
            prop::strategy::Just(GovAction::Information)
        }

        prop_oneof![
            any_action_parameter_change(),
            any_hardfork_initiation(),
            any_treasury_withdrawals(),
            any_no_confidence(),
            any_committee_update(),
            any_new_constitution(),
            any_nice_poll(),
        ]
    }

    prop_compose! {
        pub fn any_withdrawal()(
            reward_account in any_reward_account(),
            amount in any::<Lovelace>(),
        ) -> (RewardAccount, Lovelace) {
            (reward_account, amount)
        }
    }

    pub fn any_guardrails_script() -> impl Strategy<Value = Nullable<ScriptHash>> {
        any_nullable(any_script_hash())
    }

    prop_compose! {
        pub fn any_protocol_parameter()(
            protocol_version in any_protocol_version(),
            max_block_body_size in any::<u64>(),
            max_transaction_size in any::<u64>(),
            max_block_header_size in any::<u16>(),
            max_tx_ex_units in any_ex_units(),
            max_block_ex_units in any_ex_units(),
            max_value_size in any::<u64>(),
            max_collateral_inputs in any::<u16>(),
            min_fee_a in any::<Lovelace>(),
            min_fee_b in any::<Lovelace>(),
            stake_credential_deposit in any::<Lovelace>(),
            stake_pool_deposit in any::<Lovelace>(),
            monetary_expansion_rate in any_rational_number(),
            treasury_expansion_rate in any_rational_number(),
            min_pool_cost in any::<Lovelace>(),
            lovelace_per_utxo_byte in any::<Lovelace>(),
            prices in any_ex_units_prices(),
            min_fee_ref_script_lovelace_per_byte in any_rational_number(),
            stake_pool_max_retirement_epoch in any::<u64>(),
            optimal_stake_pools_count in any::<u16>(),
            pledge_influence in any_rational_number(),
            collateral_percentage in any::<u16>(),
            cost_models in any_cost_models(),
            pool_voting_thresholds in any_pool_voting_thresholds(),
            drep_voting_thresholds in any_drep_voting_thresholds(),
            min_committee_size in any::<u16>(),
            max_committee_term_length in any::<u64>(),
            gov_action_lifetime in any::<u64>(),
            gov_action_deposit in any::<Lovelace>(),
            drep_deposit in any::<Lovelace>(),
            drep_expiry in any::<u64>(),
        ) -> ProtocolParameters {
        let default = &*PREPROD_INITIAL_PROTOCOL_PARAMETERS;
        ProtocolParameters {
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
            max_ref_script_size_per_tx: default.max_ref_script_size_per_tx,
            max_ref_script_size_per_block: default.max_ref_script_size_per_block,
            ref_script_cost_stride: default.ref_script_cost_stride,
            ref_script_cost_multiplier: default.ref_script_cost_multiplier.clone(),
            stake_pool_max_retirement_epoch,
            optimal_stake_pools_count,
            pledge_influence,
            collateral_percentage,
            cost_models: Box::new(cost_models),
            pool_voting_thresholds: Box::new(pool_voting_thresholds),
            drep_voting_thresholds: Box::new(drep_voting_thresholds),
            min_committee_size,
            max_committee_term_length,
            gov_action_lifetime,
            gov_action_deposit,
            drep_deposit,
            drep_expiry,
            }
        }
    }
}
