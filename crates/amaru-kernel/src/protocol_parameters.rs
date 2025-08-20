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

use crate::{cbor, Coin, EpochInterval, ExUnits, Lovelace, RationalNumber};
use amaru_slot_arithmetic::Slot;
use pallas_codec::minicbor::{data::Tag, Decoder};
use pallas_primitives::{conway::CostModels, CostModel};

pub use default::*;
mod default;

/// Model from https://github.com/IntersectMBO/formal-ledger-specifications/blob/master/src/Ledger/PParams.lagda
/// Some of the names have been adapted to improve readability.
/// Also see https://github.com/IntersectMBO/cardano-ledger/blob/d90eb4df4651970972d860e95f1a3697a3de8977/eras/conway/impl/cddl-files/conway.cddl#L324
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolParameters {
    // Network group
    pub max_block_body_size: u32,
    pub max_tx_size: u32,
    pub max_header_size: u16,
    pub max_tx_ex_units: ExUnits,
    pub max_block_ex_units: ExUnits,
    pub max_val_size: u32,
    pub max_collateral_inputs: u16,

    // Economic group
    pub min_fee_a: Coin,
    pub min_fee_b: Coin,
    pub stake_credential_deposit: Coin,
    pub stake_pool_deposit: Coin,
    pub monetary_expansion_rate: RationalNumber,
    pub treasury_expansion_rate: RationalNumber,
    pub coins_per_utxo_byte: Coin,
    pub prices: Prices,
    pub min_fee_ref_script_coins_per_byte: RationalNumber,
    pub max_ref_script_size_per_tx: u32,
    pub max_ref_script_size_per_block: u32,
    pub ref_script_cost_stride: u32,
    pub ref_script_cost_multiplier: RationalNumber,

    // Technical group
    pub stake_pool_max_retirement_epoch: EpochInterval,
    pub optimal_stake_pools_count: u16,
    pub pledge_influence: RationalNumber,
    pub collateral_percentage: u16,
    pub cost_models: CostModels,

    // Governance group
    pub pool_thresholds: PoolThresholds,
    pub drep_thresholds: DrepThresholds,
    pub cc_min_size: u16,
    pub cc_max_term_length: EpochInterval,
    pub gov_action_lifetime: EpochInterval,
    pub gov_action_deposit: Coin,
    pub drep_deposit: Coin,
    pub drep_expiry: EpochInterval,
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
    d.array()?;
    let numerator = d.u64()?;
    let denominator = d.u64()?;
    Ok(RationalNumber {
        numerator,
        denominator,
    })
}

impl<'b, C> cbor::decode::Decode<'b, C> for ProtocolParameters {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        let min_fee_a = d.u64()?;
        let min_fee_b = d.u64()?;
        let max_block_body_size = d.u32()?;
        let max_tx_size = d.u32()?;
        let max_header_size = d.u16()?;
        let stake_credential_deposit = d.u64()?;
        let stake_pool_deposit = d.u64()?;
        let stake_pool_max_retirement_epoch = d.u32()?;
        let optimal_stake_pools_count = d.u16()?;
        let pledge_influence = decode_rationale(d)?;
        let monetary_expansion_rate = decode_rationale(d)?;

        let _ = decode_rationale(d)?; // TODO unknown 1  5
        let _ = d.array()?;
        d.u8()?;
        d.u8()?; // TODO unknown 9  0
        let _ = d.u32()?; // TODO unknown 170000000

        let coins_per_utxo_byte = d.u64()?;

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
        d.array()?;
        let prices = Prices {
            mem: decode_rationale(d)?,
            step: decode_rationale(d)?,
        };
        d.array()?;
        let max_tx_ex_units = ExUnits {
            mem: d.u32()? as u64,
            steps: d.u64()?,
        };
        d.array()?;
        let max_block_ex_units = ExUnits {
            mem: d.u64()?,
            steps: d.u64()?,
        };
        let max_val_size = d.u32()?;
        let collateral_percentage = d.u16()?;
        let max_collateral_inputs = d.u16()?;

        // TODO validate order
        d.array()?;
        let pool_thresholds = PoolThresholds {
            no_confidence: decode_rationale(d)?,
            committee: decode_rationale(d)?,
            committee_under_no_confidence: decode_rationale(d)?,
            hard_fork: decode_rationale(d)?,
            security_group: decode_rationale(d)?,
        };
        // TODO validate order
        d.array()?;
        let drep_thresholds = DrepThresholds {
            no_confidence: decode_rationale(d)?,
            committee: decode_rationale(d)?,
            committee_under_no_confidence: decode_rationale(d)?,
            constitution: decode_rationale(d)?,
            hard_fork: decode_rationale(d)?,
            protocol_parameters: ProtocolParametersThresholds {
                network_group: decode_rationale(d)?,
                economic_group: decode_rationale(d)?,
                technical_group: decode_rationale(d)?,
                governance_group: decode_rationale(d)?,
            },
            treasury_withdrawal: decode_rationale(d)?,
        };
        let cc_min_size = d.u16()?;
        let cc_max_term_length = d.u32()?;
        let gov_action_lifetime = d.u32()?;
        let gov_action_deposit = d.u64()?;
        let drep_deposit = d.u64()?;
        let drep_expiry = d.decode_with(ctx)?;
        let min_fee_ref_script_coins_per_byte = decode_rationale(d)?;

        Ok(ProtocolParameters {
            min_fee_a,
            min_fee_b,
            max_block_body_size,
            max_tx_size,
            max_header_size,
            stake_credential_deposit,
            stake_pool_deposit,
            stake_pool_max_retirement_epoch,
            optimal_stake_pools_count,
            pledge_influence,
            monetary_expansion_rate,
            coins_per_utxo_byte,
            cost_models: CostModels {
                plutus_v1,
                plutus_v2,
                plutus_v3,
            },
            prices,
            max_tx_ex_units,
            max_block_ex_units,
            max_val_size,
            collateral_percentage,
            max_collateral_inputs,
            pool_thresholds,
            drep_thresholds,
            cc_min_size,
            cc_max_term_length,
            gov_action_lifetime,
            gov_action_deposit,
            drep_deposit,
            drep_expiry,
            min_fee_ref_script_coins_per_byte,
            max_ref_script_size_per_tx: 200 * 1024, //Hardcoded in the haskell ledger (https://github.com/IntersectMBO/cardano-ledger/blob/3fe73a26588876bbf033bf4c4d25c97c2d8564dd/eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Ledger.hs#L154)
            max_ref_script_size_per_block: 1024 * 1024, // Hardcoded in the haskell ledger (https://github.com/IntersectMBO/cardano-ledger/blob/3fe73a26588876bbf033bf4c4d25c97c2d8564dd/eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Bbody.hs#L91)
            ref_script_cost_stride: 25600, // Hardcoded in the haskell ledger (https://github.com/IntersectMBO/cardano-ledger/blob/3fe73a26588876bbf033bf4c4d25c97c2d8564dd/eras/conway/impl/src/Cardano/Ledger/Conway/Tx.hs#L82)
            ref_script_cost_multiplier: RationalNumber {
                numerator: 12,
                denominator: 10,
            }, // Hardcoded in the haskell ledger (https://github.com/IntersectMBO/cardano-ledger/blob/3fe73a26588876bbf033bf4c4d25c97c2d8564dd/eras/conway/impl/src/Cardano/Ledger/Conway/Tx.hs#L85)
            treasury_expansion_rate: RationalNumber {
                numerator: 2,
                denominator: 10,
            },
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

impl<C> cbor::encode::Encode<C> for ProtocolParameters {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(31)?;
        e.u64(self.min_fee_a)?;
        e.u64(self.min_fee_b)?;
        e.u32(self.max_block_body_size)?;
        e.u32(self.max_tx_size)?;
        e.u16(self.max_header_size)?;
        e.u64(self.stake_credential_deposit)?;
        e.u64(self.stake_pool_deposit)?;
        e.u32(self.stake_pool_max_retirement_epoch)?;
        e.u16(self.optimal_stake_pools_count)?;
        encode_rationale(e, &self.pledge_influence)?;
        encode_rationale(e, &self.monetary_expansion_rate)?;

        encode_rationale(
            e,
            &RationalNumber {
                numerator: 0,
                denominator: 0,
            },
        )?;
        e.array(2)?;
        e.u8(0)?;
        e.u8(0)?;
        e.u32(0)?;

        e.u64(self.coins_per_utxo_byte)?;

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

        e.array(2)?;
        encode_rationale(e, &self.prices.mem)?;
        encode_rationale(e, &self.prices.step)?;

        e.array(2)?;
        e.u64(self.max_tx_ex_units.mem)?;
        e.u64(self.max_tx_ex_units.steps)?;

        e.array(2)?;
        e.u64(self.max_block_ex_units.mem)?;
        e.u64(self.max_block_ex_units.steps)?;

        e.u32(self.max_val_size)?;
        e.u16(self.collateral_percentage)?;
        e.u16(self.max_collateral_inputs)?;

        // TODO validate order
        e.array(5)?;
        encode_rationale(e, &self.pool_thresholds.no_confidence)?;
        encode_rationale(e, &self.pool_thresholds.committee)?;
        encode_rationale(e, &self.pool_thresholds.committee_under_no_confidence)?;
        encode_rationale(e, &self.pool_thresholds.hard_fork)?;
        encode_rationale(e, &self.pool_thresholds.security_group)?;

        // TODO validate order
        e.array(10)?;
        encode_rationale(e, &self.drep_thresholds.no_confidence)?;
        encode_rationale(e, &self.drep_thresholds.committee)?;
        encode_rationale(e, &self.drep_thresholds.committee_under_no_confidence)?;
        encode_rationale(e, &self.drep_thresholds.constitution)?;
        encode_rationale(e, &self.drep_thresholds.hard_fork)?;
        encode_rationale(e, &self.drep_thresholds.protocol_parameters.network_group)?;
        encode_rationale(e, &self.drep_thresholds.protocol_parameters.economic_group)?;
        encode_rationale(e, &self.drep_thresholds.protocol_parameters.technical_group)?;
        encode_rationale(
            e,
            &self.drep_thresholds.protocol_parameters.governance_group,
        )?;
        encode_rationale(e, &self.drep_thresholds.treasury_withdrawal)?;

        e.u16(self.cc_min_size)?;
        e.u32(self.cc_max_term_length)?;
        e.u32(self.gov_action_lifetime)?;
        e.u64(self.gov_action_deposit)?;
        e.u64(self.drep_deposit)?;
        e.encode_with(self.drep_expiry, ctx)?;
        encode_rationale(e, &self.min_fee_ref_script_coins_per_byte)?;

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prices {
    pub mem: RationalNumber,
    pub step: RationalNumber,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolThresholds {
    // named `q1` in the spec
    pub no_confidence: RationalNumber,
    // named `q2a` in the spec
    pub committee: RationalNumber,
    // named `q2b` in the spec
    pub committee_under_no_confidence: RationalNumber,
    // named `q4` in the spec
    pub hard_fork: RationalNumber,
    // named `q5e` in the spec
    pub security_group: RationalNumber,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrepThresholds {
    // named `p1` in the spec
    pub no_confidence: RationalNumber,
    // named `p2` in the spec
    pub committee: RationalNumber,
    // named `p2b` in the spec
    pub committee_under_no_confidence: RationalNumber,
    // named `p3` in the spec
    pub constitution: RationalNumber,
    // named `p4` in the spec
    pub hard_fork: RationalNumber,
    pub protocol_parameters: ProtocolParametersThresholds,
    // named `p6` in the spec
    pub treasury_withdrawal: RationalNumber,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolParametersThresholds {
    // named `p5a` in the spec
    pub network_group: RationalNumber,
    // named `p5b` in the spec
    pub economic_group: RationalNumber,
    // named `p5c` in the spec
    pub technical_group: RationalNumber,
    // named `p5d` in the spec
    pub governance_group: RationalNumber,
}

#[cfg(test)]
pub(crate) mod test {
    use super::PREPROD_INITIAL_PROTOCOL_PARAMETERS;
    use crate::{
        prop_cbor_roundtrip,
        protocol_parameters::{
            CostModels, DrepThresholds, PoolThresholds, Prices, ProtocolParameters,
            ProtocolParametersThresholds,
        },
        Coin, ExUnits, RationalNumber,
    };
    use proptest::prelude::*;

    prop_cbor_roundtrip!(ProtocolParameters, any_protocol_paramater());

    prop_compose! {
        fn any_rational_number()(numerator in any::<u64>(), denominator in any::<u64>()) -> RationalNumber {
            RationalNumber {
                numerator,
                denominator,
            }
        }
    }

    prop_compose! {
        fn any_ex_units()(
            mem in any::<u32>(),
            steps in any::<u64>(),
        ) -> ExUnits {
            ExUnits {
                mem: mem as u64,
                steps,
            }
        }
    }

    prop_compose! {
        fn any_cost_models()(
            plutus_v1 in any::<Vec<i64>>(),
            plutus_v2 in any::<Vec<i64>>(),
            plutus_v3 in any::<Vec<i64>>(),
        ) -> CostModels {
            CostModels {
                plutus_v1: Some(plutus_v1),
                plutus_v2: Some(plutus_v2),
                plutus_v3: Some(plutus_v3),
            }
        }
    }

    prop_compose! {
        fn any_pool_thresholds()(
            no_confidence in any_rational_number(),
            committee in any_rational_number(),
            committee_under_no_confidence in any_rational_number(),
            hard_fork in any_rational_number(),
            security_group in any_rational_number(),
        ) -> PoolThresholds {
            PoolThresholds {
                no_confidence,
                committee,
                committee_under_no_confidence,
                hard_fork,
                security_group,
            }
        }
    }

    prop_compose! {
        fn any_drep_thresholds()(
            no_confidence in any_rational_number(),
            committee in any_rational_number(),
            committee_under_no_confidence in any_rational_number(),
            constitution in any_rational_number(),
            hard_fork in any_rational_number(),
            network_group in any_rational_number(),
            economic_group in any_rational_number(),
            technical_group in any_rational_number(),
            governance_group in any_rational_number(),
            treasury_withdrawal in any_rational_number(),
        ) -> DrepThresholds {
            DrepThresholds {
                no_confidence,
                committee,
                committee_under_no_confidence,
                constitution,
                hard_fork,
                protocol_parameters: ProtocolParametersThresholds {
                    network_group,
                    economic_group,
                    technical_group,
                    governance_group,
                },
                treasury_withdrawal,
            }
        }
    }

    prop_compose! {
        fn any_prices()(
            mem in any_rational_number(),
            step in any_rational_number(),
        ) -> Prices {
            Prices {
                mem,
                step,
            }
        }
    }

    prop_compose! {
        fn any_protocol_paramater()(
            max_block_body_size in any::<u32>(),
            max_tx_size in any::<u32>(),
            max_header_size in any::<u16>(),
            max_tx_ex_units in any_ex_units(),
            max_block_ex_units in any_ex_units(),
            max_val_size in any::<u32>(),
            max_collateral_inputs in any::<u16>(),
            min_fee_a in any::<Coin>(),
            min_fee_b in any::<Coin>(),
            stake_credential_deposit in any::<Coin>(),
            stake_pool_deposit in any::<Coin>(),
            monetary_expansion_rate in any_rational_number(),
            coins_per_utxo_byte in any::<Coin>(),
            prices in any_prices(),
            min_fee_ref_script_coins_per_byte in any_rational_number(),
            stake_pool_max_retirement_epoch in any::<u32>(),
            optimal_stake_pools_count in any::<u16>(),
            pledge_influence in any_rational_number(),
            collateral_percentage in any::<u16>(),
            cost_models in any_cost_models(),
            pool_thresholds in any_pool_thresholds(),
            drep_thresholds in any_drep_thresholds(),
            cc_min_size in any::<u16>(),
            cc_max_term_length in any::<u32>(),
            gov_action_lifetime in any::<u32>(),
            gov_action_deposit in any::<Coin>(),
            drep_deposit in any::<Coin>(),
            drep_expiry in any::<u32>(),
        ) -> ProtocolParameters {
        let default = &*PREPROD_INITIAL_PROTOCOL_PARAMETERS;
        ProtocolParameters {
            max_block_body_size,
            max_tx_size,
            max_header_size,
            max_tx_ex_units,
            max_block_ex_units,
            max_val_size,
            max_collateral_inputs,
            min_fee_a,
            min_fee_b,
            stake_credential_deposit,
            stake_pool_deposit,
            monetary_expansion_rate,
            treasury_expansion_rate: default.treasury_expansion_rate.clone(),
            coins_per_utxo_byte,
            prices,
            min_fee_ref_script_coins_per_byte,
            max_ref_script_size_per_tx: default.max_ref_script_size_per_tx,
            max_ref_script_size_per_block: default.max_ref_script_size_per_block,
            ref_script_cost_stride: default.ref_script_cost_stride,
            ref_script_cost_multiplier: default.ref_script_cost_multiplier.clone(),
            stake_pool_max_retirement_epoch,
            optimal_stake_pools_count,
            pledge_influence,
            collateral_percentage,
            cost_models,
            pool_thresholds,
            drep_thresholds,
            cc_min_size,
            cc_max_term_length,
            gov_action_lifetime,
            gov_action_deposit,
            drep_deposit,
            drep_expiry,
            }
        }
    }
}
