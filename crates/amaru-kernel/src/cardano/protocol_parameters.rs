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

use crate::{
    CostModel, CostModels, DRepVotingThresholds, ExUnitPrices, ExUnits, Lovelace, PlutusVersion, PoolVotingThresholds,
    ProtocolParamUpdate, ProtocolVersion, RationalNumber, cbor,
};

mod default;
pub use default::*;

/// Model from <https://github.com/IntersectMBO/formal-ledger-specifications/blob/master/src/Ledger/PParams.lagda>
/// Some of the names have been adapted to improve readability.
/// Also see <https://github.com/IntersectMBO/cardano-ledger/blob/d90eb4df4651970972d860e95f1a3697a3de8977/eras/conway/impl/cddl-files/conway.cddl#L324>
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
    pub stake_pool_max_retirement_epoch: u64,
    pub optimal_stake_pools_count: u16,
    pub pledge_influence: RationalNumber,
    pub collateral_percentage: u16,
    pub cost_models: CostModels,

    // Governance group
    pub pool_voting_thresholds: PoolVotingThresholds,
    pub drep_voting_thresholds: DRepVotingThresholds,
    pub min_committee_size: u16,
    pub max_committee_term_length: u64,
    pub gov_action_lifetime: u64,
    pub gov_action_deposit: Lovelace,
    pub drep_deposit: Lovelace,
    pub drep_expiry: u64,
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
            // FIXME(cbor): update in Pallas; should be a u16
            u.max_block_header_size.map(|x| x as u16),
        );
        set(&mut self.stake_credential_deposit, u.key_deposit);
        set(&mut self.stake_pool_deposit, u.pool_deposit);
        set(&mut self.stake_pool_max_retirement_epoch, u.maximum_epoch);
        set(
            &mut self.optimal_stake_pools_count,
            // FIXME(cbor): update in Pallas; should be a u16
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
            match PlutusVersion::V1 {
                PlutusVersion::V1 => {
                    if let Some(plutus_v1) = cost_models.plutus_v1 {
                        self.cost_models.plutus_v1 = Some(plutus_v1);
                    }
                }
                PlutusVersion::V2 | PlutusVersion::V3 => (),
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
            // FIXME(cbor): update in Pallas; should be a u16
            u.collateral_percentage.map(|x| x as u16),
        );
        set(
            &mut self.max_collateral_inputs,
            // FIXME(cbor): update in Pallas; should be a u16
            u.max_collateral_inputs.map(|x| x as u16),
        );
        set(&mut self.pool_voting_thresholds, u.pool_voting_thresholds);
        set(&mut self.drep_voting_thresholds, u.drep_voting_thresholds);
        set(
            &mut self.min_committee_size,
            // FIXME(cbor): update in Pallas; should be a u16
            u.min_committee_size.map(|x| x as u16),
        );
        set(&mut self.max_committee_term_length, u.committee_term_limit);
        set(&mut self.gov_action_lifetime, u.governance_action_validity_period);
        set(&mut self.gov_action_deposit, u.governance_action_deposit);
        set(&mut self.drep_deposit, u.drep_deposit);
        set(&mut self.drep_expiry, u.drep_inactivity_period);
        set(&mut self.min_fee_ref_script_lovelace_per_byte, u.minfee_refscript_cost_per_byte);
    }
}

#[cfg(any(test, feature = "test-utils"))]
mod fixture {
    use std::fmt;

    use serde::de::{Error, IgnoredAny, MapAccess, Visitor};

    use super::{
        CostModels, DRepVotingThresholds, ExUnitPrices, ExUnits, Lovelace, PoolVotingThresholds, ProtocolParameters,
        ProtocolVersion, RationalNumber,
    };

    // NOTE: Hand-written deserializer for the protocol parameters fixture
    //
    // The fixture wire format intentionally follows Ogmios-inspired, snake_cased field names
    // (e.g. `min_fee_coefficient` for `min_fee_a`) and groups the reference-script parameters
    // under a nested `min_fee_reference_scripts` object. That vocabulary is a stable contract
    // shared with other fixture consumers, so instead of deriving an instance from the Rust
    // field names, this implementation maps fixture keys onto `ProtocolParameters` by hand.
    impl<'de> serde::Deserialize<'de> for ProtocolParameters {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            #[derive(serde::Deserialize)]
            struct MinFeeReferenceScripts {
                range: u32,
                base: RationalNumber,
                multiplier: RationalNumber,
            }

            struct FixtureVisitor;

            impl<'de> Visitor<'de> for FixtureVisitor {
                type Value = ProtocolParameters;

                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("a protocol parameters fixture object")
                }

                fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                    let mut protocol_version: Option<ProtocolVersion> = None;
                    let mut min_fee_a: Option<Lovelace> = None;
                    let mut min_fee_b: Option<u64> = None;
                    let mut min_fee_reference_scripts: Option<MinFeeReferenceScripts> = None;
                    let mut lovelace_per_utxo_byte: Option<Lovelace> = None;
                    let mut max_block_body_size: Option<u64> = None;
                    let mut max_block_header_size: Option<u16> = None;
                    let mut max_transaction_size: Option<u64> = None;
                    let mut max_value_size: Option<u64> = None;
                    let mut max_ref_script_size_per_tx: Option<u32> = None;
                    let mut stake_credential_deposit: Option<Lovelace> = None;
                    let mut stake_pool_deposit: Option<Lovelace> = None;
                    let mut stake_pool_max_retirement_epoch: Option<u64> = None;
                    let mut pledge_influence: Option<RationalNumber> = None;
                    let mut min_pool_cost: Option<u64> = None;
                    let mut optimal_stake_pools_count: Option<u16> = None;
                    let mut monetary_expansion_rate: Option<RationalNumber> = None;
                    let mut treasury_expansion_rate: Option<RationalNumber> = None;
                    let mut collateral_percentage: Option<u16> = None;
                    let mut max_collateral_inputs: Option<u16> = None;
                    let mut cost_models: Option<CostModels> = None;
                    let mut prices: Option<ExUnitPrices> = None;
                    let mut max_tx_ex_units: Option<ExUnits> = None;
                    let mut max_block_ex_units: Option<ExUnits> = None;
                    let mut pool_voting_thresholds: Option<PoolVotingThresholds> = None;
                    let mut min_committee_size: Option<u16> = None;
                    let mut max_committee_term_length: Option<u64> = None;
                    let mut gov_action_lifetime: Option<u64> = None;
                    let mut gov_action_deposit: Option<Lovelace> = None;
                    let mut drep_voting_thresholds: Option<DRepVotingThresholds> = None;
                    let mut drep_deposit: Option<Lovelace> = None;
                    let mut drep_expiry: Option<u64> = None;

                    macro_rules! set {
                        ($slot:ident, $key:literal) => {{
                            if $slot.is_some() {
                                return Err(A::Error::duplicate_field($key));
                            }
                            $slot = Some(map.next_value()?);
                        }};
                    }

                    while let Some(key) = map.next_key::<String>()? {
                        match key.as_str() {
                            "version" => set!(protocol_version, "version"),
                            "min_fee_coefficient" => set!(min_fee_a, "min_fee_coefficient"),
                            "min_fee_constant" => set!(min_fee_b, "min_fee_constant"),
                            "min_fee_reference_scripts" => {
                                set!(min_fee_reference_scripts, "min_fee_reference_scripts")
                            }
                            "min_utxo_deposit_coefficient" => {
                                set!(lovelace_per_utxo_byte, "min_utxo_deposit_coefficient")
                            }
                            "max_block_body_size" => set!(max_block_body_size, "max_block_body_size"),
                            "max_block_header_size" => set!(max_block_header_size, "max_block_header_size"),
                            "max_transaction_size" => set!(max_transaction_size, "max_transaction_size"),
                            "max_value_size" => set!(max_value_size, "max_value_size"),
                            "max_reference_scripts_size" => {
                                set!(max_ref_script_size_per_tx, "max_reference_scripts_size")
                            }
                            "stake_credential_deposit" => set!(stake_credential_deposit, "stake_credential_deposit"),
                            "stake_pool_deposit" => set!(stake_pool_deposit, "stake_pool_deposit"),
                            "stake_pool_retirement_epoch_bound" => {
                                set!(stake_pool_max_retirement_epoch, "stake_pool_retirement_epoch_bound")
                            }
                            "stake_pool_pledge_influence" => set!(pledge_influence, "stake_pool_pledge_influence"),
                            "min_stake_pool_cost" => set!(min_pool_cost, "min_stake_pool_cost"),
                            "desired_number_of_stake_pools" => {
                                set!(optimal_stake_pools_count, "desired_number_of_stake_pools")
                            }
                            "monetary_expansion" => set!(monetary_expansion_rate, "monetary_expansion"),
                            "treasury_expansion" => set!(treasury_expansion_rate, "treasury_expansion"),
                            "collateral_percentage" => set!(collateral_percentage, "collateral_percentage"),
                            "max_collateral_inputs" => set!(max_collateral_inputs, "max_collateral_inputs"),
                            "plutus_cost_models" => set!(cost_models, "plutus_cost_models"),
                            "script_execution_prices" => set!(prices, "script_execution_prices"),
                            "max_execution_units_per_transaction" => {
                                set!(max_tx_ex_units, "max_execution_units_per_transaction")
                            }
                            "max_execution_units_per_block" => {
                                set!(max_block_ex_units, "max_execution_units_per_block")
                            }
                            "stake_pool_voting_thresholds" => {
                                set!(pool_voting_thresholds, "stake_pool_voting_thresholds")
                            }
                            "constitutional_committee_min_size" => {
                                set!(min_committee_size, "constitutional_committee_min_size")
                            }
                            "constitutional_committee_max_term_length" => {
                                set!(max_committee_term_length, "constitutional_committee_max_term_length")
                            }
                            "governance_action_lifetime" => set!(gov_action_lifetime, "governance_action_lifetime"),
                            "governance_action_deposit" => set!(gov_action_deposit, "governance_action_deposit"),
                            "delegate_representative_voting_thresholds" => {
                                set!(drep_voting_thresholds, "delegate_representative_voting_thresholds")
                            }
                            "delegate_representative_deposit" => {
                                set!(drep_deposit, "delegate_representative_deposit")
                            }
                            "delegate_representative_max_idle_time" => {
                                set!(drep_expiry, "delegate_representative_max_idle_time")
                            }
                            _ => {
                                map.next_value::<IgnoredAny>()?;
                            }
                        }
                    }

                    macro_rules! require {
                        ($slot:ident, $key:literal) => {
                            $slot.ok_or_else(|| A::Error::missing_field($key))?
                        };
                    }

                    let min_fee_reference_scripts = require!(min_fee_reference_scripts, "min_fee_reference_scripts");

                    Ok(ProtocolParameters {
                        protocol_version: require!(protocol_version, "version"),
                        min_fee_a: require!(min_fee_a, "min_fee_coefficient"),
                        min_fee_b: require!(min_fee_b, "min_fee_constant"),
                        max_block_body_size: require!(max_block_body_size, "max_block_body_size"),
                        max_transaction_size: require!(max_transaction_size, "max_transaction_size"),
                        max_block_header_size: require!(max_block_header_size, "max_block_header_size"),
                        stake_credential_deposit: require!(stake_credential_deposit, "stake_credential_deposit"),
                        stake_pool_deposit: require!(stake_pool_deposit, "stake_pool_deposit"),
                        stake_pool_max_retirement_epoch: require!(
                            stake_pool_max_retirement_epoch,
                            "stake_pool_retirement_epoch_bound"
                        ),
                        optimal_stake_pools_count: require!(optimal_stake_pools_count, "desired_number_of_stake_pools"),
                        pledge_influence: require!(pledge_influence, "stake_pool_pledge_influence"),
                        monetary_expansion_rate: require!(monetary_expansion_rate, "monetary_expansion"),
                        treasury_expansion_rate: require!(treasury_expansion_rate, "treasury_expansion"),
                        min_pool_cost: require!(min_pool_cost, "min_stake_pool_cost"),
                        lovelace_per_utxo_byte: require!(lovelace_per_utxo_byte, "min_utxo_deposit_coefficient"),
                        prices: require!(prices, "script_execution_prices"),
                        max_tx_ex_units: require!(max_tx_ex_units, "max_execution_units_per_transaction"),
                        max_block_ex_units: require!(max_block_ex_units, "max_execution_units_per_block"),
                        max_value_size: require!(max_value_size, "max_value_size"),
                        collateral_percentage: require!(collateral_percentage, "collateral_percentage"),
                        max_collateral_inputs: require!(max_collateral_inputs, "max_collateral_inputs"),
                        pool_voting_thresholds: require!(pool_voting_thresholds, "stake_pool_voting_thresholds"),
                        drep_voting_thresholds: require!(
                            drep_voting_thresholds,
                            "delegate_representative_voting_thresholds"
                        ),
                        min_committee_size: require!(min_committee_size, "constitutional_committee_min_size"),
                        max_committee_term_length: require!(
                            max_committee_term_length,
                            "constitutional_committee_max_term_length"
                        ),
                        gov_action_lifetime: require!(gov_action_lifetime, "governance_action_lifetime"),
                        gov_action_deposit: require!(gov_action_deposit, "governance_action_deposit"),
                        drep_deposit: require!(drep_deposit, "delegate_representative_deposit"),
                        drep_expiry: require!(drep_expiry, "delegate_representative_max_idle_time"),
                        min_fee_ref_script_lovelace_per_byte: min_fee_reference_scripts.base,
                        cost_models: require!(cost_models, "plutus_cost_models"),
                        max_ref_script_size_per_tx: require!(max_ref_script_size_per_tx, "max_reference_scripts_size"),
                        // Hardcoded in the Haskell ledger; not yet a real protocol parameter, so the
                        // fixture schema does not carry it.
                        max_ref_script_size_per_block: 1024 * 1024,
                        ref_script_cost_stride: min_fee_reference_scripts.range,
                        ref_script_cost_multiplier: min_fee_reference_scripts.multiplier,
                    })
                }
            }

            d.deserialize_map(FixtureVisitor)
        }
    }
}

fn decode_rationale(d: &mut cbor::Decoder<'_>) -> Result<RationalNumber, cbor::decode::Error> {
    cbor::allow_tag(d, cbor::Tag::new(30))?;
    cbor::heterogeneous_array(d, |d, assert_len| {
        assert_len(2)?;
        let numerator = d.u64()?;
        let denominator = d.u64()?;
        Ok(RationalNumber { numerator, denominator })
    })
}

impl<'b, C: cbor::HasProtocolVersion> cbor::decode::Decode<'b, C> for ProtocolParameters {
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
        let protocol_version = d.decode_with(ctx)?;
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
            cost_models: CostModels { plutus_v1, plutus_v2, plutus_v3 },
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
            // Hardcoded in the haskell ledger
            // <https://github.com/IntersectMBO/cardano-ledger/blob/3fe73a26588876bbf033bf4c4d25c97c2d8564dd/eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Ledger.hs#L154>
            max_ref_script_size_per_tx: 200 * 1024,
            // Hardcoded in the haskell ledger
            // <https://github.com/IntersectMBO/cardano-ledger/blob/3fe73a26588876bbf033bf4c4d25c97c2d8564dd/eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Bbody.hs#L91>
            max_ref_script_size_per_block: 1024 * 1024,
            // Hardcoded in the haskell ledger
            // <https://github.com/IntersectMBO/cardano-ledger/blob/3fe73a26588876bbf033bf4c4d25c97c2d8564dd/eras/conway/impl/src/Cardano/Ledger/Conway/Tx.hs#L82>
            ref_script_cost_stride: 25600,
            // Hardcoded in the haskell ledger
            // <https://github.com/IntersectMBO/cardano-ledger/blob/3fe73a26588876bbf033bf4c4d25c97c2d8564dd/eras/conway/impl/src/Cardano/Ledger/Conway/Tx.hs#L85>
            ref_script_cost_multiplier: RationalNumber { numerator: 12, denominator: 10 },
        })
    }
}

fn encode_rationale<W: cbor::encode::Write>(
    e: &mut cbor::Encoder<W>,
    rat: &RationalNumber,
) -> Result<(), cbor::encode::Error<W::Error>> {
    e.tag(cbor::Tag::new(30))?;
    e.array(2)?;

    e.u64(rat.numerator)?;
    e.u64(rat.denominator)?;
    Ok(())
}

impl<C: cbor::HasProtocolVersion> cbor::encode::Encode<C> for ProtocolParameters {
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
        e.encode_with(self.protocol_version, ctx)?;
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

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::{collection, option, prelude::*};

    use super::PREPROD_DEFAULT_PROTOCOL_PARAMETERS;
    use crate::{
        CostModel, CostModels, Credential, DRepVotingThresholds, Epoch, ExUnitPrices, ExUnits, GovernanceAction, Hash,
        KeyValuePairs, Lovelace, PoolVotingThresholds, ProposalId, ProtocolParamUpdate, ProtocolParameters,
        ProtocolVersion, RewardAccount, any_constitution, any_credential, any_epoch, any_hash28, any_proposal_id,
        any_rational_number, any_reward_account, size::SCRIPT,
    };

    #[cfg(not(target_os = "windows"))]
    crate::prop_cbor_roundtrip!(ProtocolParameters, any_protocol_parameter());

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
            ProtocolVersion::new((major % 13) as u64, minor)
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

    pub fn any_gov_action() -> impl Strategy<Value = GovernanceAction> {
        prop_compose! {
            fn any_parent_proposal_id()(
                proposal_id in option::of(any_proposal_id()),
            ) -> Option<ProposalId> {
                proposal_id
            }
        }

        prop_compose! {
            fn any_action_parameter_change()(
                parent_proposal_id in any_parent_proposal_id(),
                pparams in any_protocol_params_update(),
                guardrails in any_guardrails_script(),
            ) -> GovernanceAction {
                GovernanceAction::ParameterChange(parent_proposal_id, Box::new(pparams), guardrails)
            }
        }

        prop_compose! {
            fn any_hardfork_initiation()(
                parent_proposal_id in any_parent_proposal_id(),
                protocol_version in any_protocol_version(),
            ) -> GovernanceAction {
                GovernanceAction::HardForkInitiation(parent_proposal_id, protocol_version)
            }
        }

        prop_compose! {
            #[allow(clippy::unwrap_used)]
            fn any_treasury_withdrawals()(
                withdrawals in collection::vec(any_withdrawal(), 0..3),
                guardrails in any_guardrails_script(),
            ) -> GovernanceAction {
                GovernanceAction::TreasuryWithdrawals(
                    KeyValuePairs::try_from(withdrawals).unwrap(),
                    guardrails
                )
            }
        }

        prop_compose! {
            fn any_no_confidence()(
                parent_proposal_id in any_parent_proposal_id(),
            ) -> GovernanceAction {
                GovernanceAction::NoConfidence(parent_proposal_id)
            }
        }

        prop_compose! {
            fn any_committee_registration()(
                credential in any_credential(),
                epoch in any_epoch(),
            ) -> (Credential, Epoch) {
                (credential, epoch)
            }
        }

        prop_compose! {
            #[allow(clippy::unwrap_used)]
            fn any_committee_update()(
                parent_proposal_id in any_parent_proposal_id(),
                to_remove in collection::btree_set(any_credential(), 0..3),
                to_add in collection::vec(any_committee_registration(), 0..3),
                quorum in any_rational_number(),
            ) -> GovernanceAction {
                GovernanceAction::UpdateCommittee(
                    parent_proposal_id,
                    to_remove.into_iter().collect::<Vec<_>>(),
                    KeyValuePairs::try_from(to_add).unwrap(),
                    quorum
                )
            }
        }

        prop_compose! {
            fn any_new_constitution()(
                parent_proposal_id in any_parent_proposal_id(),
                constitution in any_constitution(),
            ) -> GovernanceAction {
                GovernanceAction::NewConstitution(parent_proposal_id, constitution)
            }
        }

        fn any_nice_poll() -> impl Strategy<Value = GovernanceAction> {
            prop::strategy::Just(GovernanceAction::Information)
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

    pub fn any_guardrails_script() -> impl Strategy<Value = Option<Hash<SCRIPT>>> {
        option::of(any_hash28())
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
            let default = &*PREPROD_DEFAULT_PROTOCOL_PARAMETERS;
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
                ref_script_cost_multiplier: default.ref_script_cost_multiplier,
                stake_pool_max_retirement_epoch,
                optimal_stake_pools_count,
                pledge_influence,
                collateral_percentage,
                cost_models,
                pool_voting_thresholds,
                drep_voting_thresholds,
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
