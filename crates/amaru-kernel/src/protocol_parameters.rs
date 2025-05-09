use pallas_codec::minicbor::Decoder;

use crate::{cbor, Coin, Epoch, EpochInterval, ExUnits, Lovelace, RationalNumber};

/// Model from https://github.com/IntersectMBO/formal-ledger-specifications/blob/master/src/Ledger/PParams.lagda
/// Some of the names have been adapted to improve readability.
/// Also see https://github.com/IntersectMBO/cardano-ledger/blob/d90eb4df4651970972d860e95f1a3697a3de8977/eras/conway/impl/cddl-files/conway.cddl#L324
#[derive(Clone, Debug)]
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
    pub max_epoch: EpochInterval,
    pub optimal_stake_pools_count: u16,
    pub pledge_influence: RationalNumber,
    pub collateral_percentage: u16,
    pub cost_models: CostModels,

    // Governance group
    pub pool_thresholds: PoolThresholds,
    pub drep_thresholds: DrepThresholds,
    pub cc_min_size: u16,
    pub cc_max_term_length: EpochInterval,
    pub gov_action_lifetime: u32,
    pub gov_action_deposit: Coin,
    pub drep_deposit: Coin,
    pub drep_expiry: Epoch,
}

fn decode_u64(d: &mut Decoder<'_>) -> Result<u64, cbor::decode::Error> {
    match d.datatype()? {
        cbor::data::Type::U8 => Ok(d.u8()? as u64),
        cbor::data::Type::U16 => Ok(d.u16()? as u64),
        cbor::data::Type::U32 => Ok(d.u32()? as u64),
        _ => Err(cbor::decode::Error::message("Expected u8 or u16 or u32 for denominator")),
    }
}

fn decode_rationale(d: &mut Decoder<'_>) -> Result<RationalNumber, cbor::decode::Error> {
    d.tag()?;
    d.array()?;
    let numerator = decode_u64(d)?;
    let denominator = decode_u64(d)?;
    Ok(RationalNumber { numerator, denominator })
}

impl<'b, C> cbor::decode::Decode<'b, C> for ProtocolParameters {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        let min_fee_a = d.u8()? as Coin;
        let min_fee_b = d.u32()? as Coin;
        let max_block_body_size = d.u32()?;
        let max_tx_size = d.u16()? as u32;
        let max_header_size = d.u16()?;
        let stake_credential_deposit = d.u32()? as Coin;
        let stake_pool_deposit = d.u32()? as Coin;
        let max_epoch = d.u8()? as EpochInterval;
        let optimal_stake_pools_count = d.u16()?;
        let pledge_influence = decode_rationale(d)?;
        let monetary_expansion_rate = decode_rationale(d)?;
        
        let _ = decode_rationale(d)?; // TODO unknown 1  5
        let _ = d.array()?; d.u8()?; d.u8()?; // TODO unknown 9  0
        let _ = d.u32()?; // TODO unknown 170000000

        let coins_per_utxo_byte = d.u16()? as u64;
        
        let _ = d.map()?; d.u8()?;
        let plutus_v1 = d.decode_with(ctx)?;
        d.u8()?;
        let plutus_v2 = d.decode_with(ctx)?;
        d.u8()?;
        let plutus_v3 = d.decode_with(ctx)?;

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
            mem: d.u32()? as u64,
            steps: d.u64()?,
        };
        let max_val_size = d.u16()? as u32;
        let collateral_percentage = d.u8()? as u16;
        let max_collateral_inputs = d.u8()? as u16;
        
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
        let cc_min_size = d.u8()? as u16;
        let cc_max_term_length = d.u8()?  as u32;
        let gov_action_lifetime = d.u8()?  as u32;
        let gov_action_deposit = d.u64()?;
        let drep_deposit = d.u32()? as u64;
        let drep_expiry = d.u8()? as u64;
        let min_fee_ref_script_coins_per_byte = decode_rationale(d)?;

        Ok(ProtocolParameters {
            min_fee_a,
            min_fee_b,
            max_block_body_size,
            max_tx_size,
            max_header_size,
            stake_credential_deposit,
            stake_pool_deposit,
            max_epoch,
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

#[derive(Clone)]
pub struct GlobalParameters {
    /// The maximum depth of a rollback, also known as the security parameter 'k'.
    /// This translates down to the length of our volatile storage, containing states of the ledger
    /// which aren't yet considered final.
    pub consensus_security_param: usize,

    /// Multiplier applied to the CONSENSUS_SECURITY_PARAM to determine Shelley's epoch length.
    pub shelley_epoch_length_scale_factor: usize,

    /// Inverse of the active slot coefficient (i.e. 1/f);
    pub active_slot_coeff_inverse: usize,

    /// Multiplier applied to the CONSENSUS_SECURITY_PARAM to determine Byron's epoch length.
    pub byron_epoch_length_scale_factor: usize,

    /// Epoch number in which the network transitioned to Shelley.
    pub shelley_transition_epoch: usize,

    /// Maximum supply of Ada, in lovelace (1 Ada = 1,000,000 Lovelace)
    pub max_lovelace_supply: Lovelace,

    /// Epoch duration after which inactive Proposals are considered expired.
    pub gov_action_lifetime: u64,

    /// Number of slots for a single KES validity period.
    pub slots_per_kes_period: u64,

    /// Maximum number of KES key evolution. Combined with SLOTS_PER_KES_PERIOD, these values
    /// indicates the validity period of a KES key before a new one is required.
    pub max_kes_evolution: u8,

    /// Number of slots in a Shelley epoch
    pub shelley_epoch_length: usize,

    /// Relative slot from which data of the previous epoch can be considered stable.
    pub stability_window: usize,

    /// Number of blocks in a Byron epoch
    pub byron_epoch_length: usize,

    /// Number of slots in the Byron era
    pub byron_total_slots: usize,

    /// Number of slots at the end of each epoch which do NOT contribute randomness to the candidate
    /// nonce of the following epoch.
    pub randomness_stabilization_window: u64,
}

impl Default for GlobalParameters {
    fn default() -> Self {
        // https://cips.cardano.org/cip/CIP-9
        let consensus_security_param = 2160;
        let active_slot_coeff_inverse = 20;
        let shelley_epoch_length_scale_factor = 10;
        let shelley_epoch_length = active_slot_coeff_inverse
            * shelley_epoch_length_scale_factor
            * consensus_security_param;
        let byron_epoch_length_scale_factor = 10;
        let byron_epoch_length = byron_epoch_length_scale_factor * consensus_security_param;
        let shelley_transition_epoch = 4;
        Self {
            consensus_security_param,
            shelley_epoch_length_scale_factor,
            active_slot_coeff_inverse,
            byron_epoch_length_scale_factor,
            shelley_transition_epoch,
            max_lovelace_supply: 45_000_000_000_000_000,
            gov_action_lifetime: 6,
            slots_per_kes_period: 129_600,
            max_kes_evolution: 62,
            shelley_epoch_length,
            stability_window: active_slot_coeff_inverse * consensus_security_param * 2,
            byron_epoch_length,
            byron_total_slots: byron_epoch_length * shelley_transition_epoch,
            randomness_stabilization_window: (4
                * consensus_security_param
                * active_slot_coeff_inverse) as u64,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Prices {
    pub mem: RationalNumber,
    pub step: RationalNumber,
}

#[derive(Debug, Clone)]
pub struct CostModels {
    pub plutus_v1: Vec<i64>,
    pub plutus_v2: Vec<i64>,
    pub plutus_v3: Vec<i64>,
}

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
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

// Decode from snapshot CBOR. Look into pallas?
impl Default for ProtocolParameters {
    // This default is the protocol parameters on Preprod as of epoch 197
    fn default() -> Self {
        Self {
            min_fee_a: 44,
            min_fee_b: 155381,
            max_block_body_size: 90112,
            max_tx_size: 16384,
            max_header_size: 1100,
            max_tx_ex_units: ExUnits {
                mem: 14_000_000,
                steps: 10_000_000_000,
            },
            max_block_ex_units: ExUnits {
                mem: 62_000_000,
                steps: 20_000_000_000,
            },
            max_val_size: 5000,
            max_collateral_inputs: 3,
            stake_credential_deposit: 2_000_000,
            stake_pool_deposit: 500_000_000,
            coins_per_utxo_byte: 4310,
            prices: Prices {
                mem: RationalNumber {
                    numerator: 577,
                    denominator: 10_000,
                },
                step: RationalNumber {
                    numerator: 721,
                    denominator: 10_000_000,
                },
            },
            min_fee_ref_script_coins_per_byte: RationalNumber {
                numerator: 15,
                denominator: 1,
            },
            max_ref_script_size_per_tx: 200 * 1024, //Hardcoded in the haskell ledger (https://github.com/IntersectMBO/cardano-ledger/blob/3fe73a26588876bbf033bf4c4d25c97c2d8564dd/eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Ledger.hs#L154)
            max_ref_script_size_per_block: 1024 * 1024, // Hardcoded in the haskell ledger (https://github.com/IntersectMBO/cardano-ledger/blob/3fe73a26588876bbf033bf4c4d25c97c2d8564dd/eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Bbody.hs#L91)
            ref_script_cost_stride: 25600, // Hardcoded in the haskell ledger (https://github.com/IntersectMBO/cardano-ledger/blob/3fe73a26588876bbf033bf4c4d25c97c2d8564dd/eras/conway/impl/src/Cardano/Ledger/Conway/Tx.hs#L82)
            ref_script_cost_multiplier: RationalNumber {
                numerator: 12,
                denominator: 10,
            }, // Hardcoded in the haskell ledger (https://github.com/IntersectMBO/cardano-ledger/blob/3fe73a26588876bbf033bf4c4d25c97c2d8564dd/eras/conway/impl/src/Cardano/Ledger/Conway/Tx.hs#L85)
            max_epoch: 18,
            pledge_influence: RationalNumber {
                numerator: 3,
                denominator: 10,
            },
            optimal_stake_pools_count: 500,
            treasury_expansion_rate: RationalNumber {
                numerator: 2,
                denominator: 10,
            },
            monetary_expansion_rate: RationalNumber {
                numerator: 3,
                denominator: 1_000,
            },
            collateral_percentage: 150,
            cost_models: CostModels {
                plutus_v1: vec![
                    100788, 420, 1, 1, 1000, 173, 0, 1, 1000, 59957, 4, 1, 11183, 32, 201305, 8356,
                    4, 16000, 100, 16000, 100, 16000, 100, 16000, 100, 16000, 100, 16000, 100, 100,
                    100, 16000, 100, 94375, 32, 132994, 32, 61462, 4, 72010, 178, 0, 1, 22151, 32,
                    91189, 769, 4, 2, 85848, 228465, 122, 0, 1, 1, 1000, 42921, 4, 2, 24548, 29498,
                    38, 1, 898148, 27279, 1, 51775, 558, 1, 39184, 1000, 60594, 1, 141895, 32,
                    83150, 32, 15299, 32, 76049, 1, 13169, 4, 22100, 10, 28999, 74, 1, 28999, 74,
                    1, 43285, 552, 1, 44749, 541, 1, 33852, 32, 68246, 32, 72362, 32, 7243, 32,
                    7391, 32, 11546, 32, 85848, 228465, 122, 0, 1, 1, 90434, 519, 0, 1, 74433, 32,
                    85848, 228465, 122, 0, 1, 1, 85848, 228465, 122, 0, 1, 1, 270652, 22588, 4,
                    1457325, 64566, 4, 20467, 1, 4, 0, 141992, 32, 100788, 420, 1, 1, 81663, 32,
                    59498, 32, 20142, 32, 24588, 32, 20744, 32, 25933, 32, 24623, 32, 53384111,
                    14333, 10,
                ],
                plutus_v2: vec![
                    100788, 420, 1, 1, 1000, 173, 0, 1, 1000, 59957, 4, 1, 11183, 32, 201305, 8356,
                    4, 16000, 100, 16000, 100, 16000, 100, 16000, 100, 16000, 100, 16000, 100, 100,
                    100, 16000, 100, 94375, 32, 132994, 32, 61462, 4, 72010, 178, 0, 1, 22151, 32,
                    91189, 769, 4, 2, 85848, 228465, 122, 0, 1, 1, 1000, 42921, 4, 2, 24548, 29498,
                    38, 1, 898148, 27279, 1, 51775, 558, 1, 39184, 1000, 60594, 1, 141895, 32,
                    83150, 32, 15299, 32, 76049, 1, 13169, 4, 22100, 10, 28999, 74, 1, 28999, 74,
                    1, 43285, 552, 1, 44749, 541, 1, 33852, 32, 68246, 32, 72362, 32, 7243, 32,
                    7391, 32, 11546, 32, 85848, 228465, 122, 0, 1, 1, 90434, 519, 0, 1, 74433, 32,
                    85848, 228465, 122, 0, 1, 1, 85848, 228465, 122, 0, 1, 1, 955506, 213312, 0, 2,
                    270652, 22588, 4, 1457325, 64566, 4, 20467, 1, 4, 0, 141992, 32, 100788, 420,
                    1, 1, 81663, 32, 59498, 32, 20142, 32, 24588, 32, 20744, 32, 25933, 32, 24623,
                    32, 43053543, 10, 53384111, 14333, 10, 43574283, 26308, 10,
                ],
                plutus_v3: vec![
                    100788, 420, 1, 1, 1000, 173, 0, 1, 1000, 59957, 4, 1, 11183, 32, 201305, 8356,
                    4, 16000, 100, 16000, 100, 16000, 100, 16000, 100, 16000, 100, 16000, 100, 100,
                    100, 16000, 100, 94375, 32, 132994, 32, 61462, 4, 72010, 178, 0, 1, 22151, 32,
                    91189, 769, 4, 2, 85848, 123203, 7305, -900, 1716, 549, 57, 85848, 0, 1, 1,
                    1000, 42921, 4, 2, 24548, 29498, 38, 1, 898148, 27279, 1, 51775, 558, 1, 39184,
                    1000, 60594, 1, 141895, 32, 83150, 32, 15299, 32, 76049, 1, 13169, 4, 22100,
                    10, 28999, 74, 1, 28999, 74, 1, 43285, 552, 1, 44749, 541, 1, 33852, 32, 68246,
                    32, 72362, 32, 7243, 32, 7391, 32, 11546, 32, 85848, 123203, 7305, -900, 1716,
                    549, 57, 85848, 0, 1, 90434, 519, 0, 1, 74433, 32, 85848, 123203, 7305, -900,
                    1716, 549, 57, 85848, 0, 1, 1, 85848, 123203, 7305, -900, 1716, 549, 57, 85848,
                    0, 1, 955506, 213312, 0, 2, 270652, 22588, 4, 1457325, 64566, 4, 20467, 1, 4,
                    0, 141992, 32, 100788, 420, 1, 1, 81663, 32, 59498, 32, 20142, 32, 24588, 32,
                    20744, 32, 25933, 32, 24623, 32, 43053543, 10, 53384111, 14333, 10, 43574283,
                    26308, 10, 16000, 100, 16000, 100, 962335, 18, 2780678, 6, 442008, 1, 52538055,
                    3756, 18, 267929, 18, 76433006, 8868, 18, 52948122, 18, 1995836, 36, 3227919,
                    12, 901022, 1, 166917843, 4307, 36, 284546, 36, 158221314, 26549, 36, 74698472,
                    36, 333849714, 1, 254006273, 72, 2174038, 72, 2261318, 64571, 4, 207616, 8310,
                    4, 1293828, 28716, 63, 0, 1, 1006041, 43623, 251, 0, 1, 100181, 726, 719, 0, 1,
                    100181, 726, 719, 0, 1, 100181, 726, 719, 0, 1, 107878, 680, 0, 1, 95336, 1,
                    281145, 18848, 0, 1, 180194, 159, 1, 1, 158519, 8942, 0, 1, 159378, 8813, 0, 1,
                    107490, 3298, 1, 106057, 655, 1, 1964219, 24520, 3,
                ],
            },
            pool_thresholds: PoolThresholds {
                no_confidence: RationalNumber {
                    numerator: 51,
                    denominator: 100,
                },
                committee: RationalNumber {
                    numerator: 51,
                    denominator: 100,
                },
                committee_under_no_confidence: RationalNumber {
                    numerator: 51,
                    denominator: 100,
                },
                hard_fork: RationalNumber {
                    numerator: 51,
                    denominator: 100,
                },
                security_group: RationalNumber {
                    numerator: 51,
                    denominator: 100,
                },
            },
            drep_thresholds: DrepThresholds {
                no_confidence: RationalNumber {
                    numerator: 51,
                    denominator: 100,
                },
                committee: RationalNumber {
                    numerator: 67,
                    denominator: 100,
                },
                committee_under_no_confidence: RationalNumber {
                    numerator: 67,
                    denominator: 100,
                },
                constitution: RationalNumber {
                    numerator: 6,
                    denominator: 10,
                },
                hard_fork: RationalNumber {
                    numerator: 75,
                    denominator: 100,
                },
                protocol_parameters: ProtocolParametersThresholds {
                    network_group: RationalNumber {
                        numerator: 6,
                        denominator: 10,
                    },
                    economic_group: RationalNumber {
                        numerator: 67,
                        denominator: 100,
                    },
                    technical_group: RationalNumber {
                        numerator: 67,
                        denominator: 100,
                    },
                    governance_group: RationalNumber {
                        numerator: 75,
                        denominator: 100,
                    },
                },
                treasury_withdrawal: RationalNumber {
                    numerator: 67,
                    denominator: 100,
                },
            },
            cc_min_size: 7,
            cc_max_term_length: 146,
            gov_action_lifetime: 6,
            gov_action_deposit: 100_000_000_000,
            drep_deposit: 500_000_000,
            drep_expiry: 20,
        }
    }
}
