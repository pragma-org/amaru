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
    ExUnitPrices, ExUnits, PROTOCOL_VERSION_9, RationalNumber, Slot,
    protocol_parameters::{
        DRepVotingThresholds, GlobalParameters, PoolVotingThresholds, ProtocolParameters,
    },
};
use pallas_primitives::conway::CostModels;
use std::sync::LazyLock;

pub static MAINNET_GLOBAL_PARAMETERS: LazyLock<GlobalParameters> = LazyLock::new(|| {
    let consensus_security_param = 2160;
    let active_slot_coeff_inverse = 20;
    let epoch_length_scale_factor = 10;
    let epoch_length =
        active_slot_coeff_inverse * epoch_length_scale_factor * consensus_security_param;
    GlobalParameters {
        consensus_security_param,
        epoch_length_scale_factor,
        active_slot_coeff_inverse,
        max_lovelace_supply: 45_000_000_000_000_000,
        slots_per_kes_period: 129_600,
        max_kes_evolution: 62,
        epoch_length,
        stability_window: Slot::from(
            (active_slot_coeff_inverse * consensus_security_param * 3) as u64,
        ),
        randomness_stabilization_window: (4 * consensus_security_param * active_slot_coeff_inverse)
            as u64,
    }
});

pub static PREPROD_GLOBAL_PARAMETERS: LazyLock<GlobalParameters> = LazyLock::new(|| {
    let consensus_security_param = 2160;
    let active_slot_coeff_inverse = 20;
    let epoch_length_scale_factor = 10;
    let epoch_length =
        active_slot_coeff_inverse * epoch_length_scale_factor * consensus_security_param;
    GlobalParameters {
        consensus_security_param,
        epoch_length_scale_factor,
        active_slot_coeff_inverse,
        max_lovelace_supply: 45_000_000_000_000_000,
        slots_per_kes_period: 129_600,
        max_kes_evolution: 62,
        epoch_length,
        stability_window: Slot::from(
            (active_slot_coeff_inverse * consensus_security_param * 3) as u64,
        ),
        randomness_stabilization_window: (4 * consensus_security_param * active_slot_coeff_inverse)
            as u64,
    }
});

// This default is the protocol parameters on Preprod as of epoch 197
pub static PREPROD_INITIAL_PROTOCOL_PARAMETERS: LazyLock<ProtocolParameters> =
    LazyLock::new(|| {
        ProtocolParameters {
            protocol_version: PROTOCOL_VERSION_9,
            min_fee_a: 44,
            min_fee_b: 155381,
            max_block_body_size: 90112,
            max_transaction_size: 16384,
            max_block_header_size: 1100,
            max_tx_ex_units: ExUnits {
                mem: 14_000_000,
                steps: 10_000_000_000,
            },
            max_block_ex_units: ExUnits {
                mem: 62_000_000,
                steps: 20_000_000_000,
            },
            max_value_size: 5000,
            max_collateral_inputs: 3,
            stake_credential_deposit: 2_000_000,
            stake_pool_deposit: 500_000_000,
            lovelace_per_utxo_byte: 4310,
            prices: ExUnitPrices {
                mem_price: RationalNumber {
                    numerator: 577,
                    denominator: 10_000,
                },
                step_price: RationalNumber {
                    numerator: 721,
                    denominator: 10_000_000,
                },
            },
            min_fee_ref_script_lovelace_per_byte: RationalNumber {
                numerator: 15,
                denominator: 1,
            },
            // Hardcoded in the haskell ledger
            // See https://github.com/IntersectMBO/cardano-ledger/blob/3fe73a26588876bbf033bf4c4d25c97c2d8564dd/eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Ledger.hs#L154
            max_ref_script_size_per_tx: 200 * 1024,

            // Hardcoded in the haskell ledger
            // See https://github.com/IntersectMBO/cardano-ledger/blob/3fe73a26588876bbf033bf4c4d25c97c2d8564dd/eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Bbody.hs#L91
            max_ref_script_size_per_block: 1024 * 1024,

            // Hardcoded in the haskell ledger
            // See https://github.com/IntersectMBO/cardano-ledger/blob/3fe73a26588876bbf033bf4c4d25c97c2d8564dd/eras/conway/impl/src/Cardano/Ledger/Conway/Tx.hs#L82
            ref_script_cost_stride: 25600,

            // Hardcoded in the haskell ledger
            // See https://github.com/IntersectMBO/cardano-ledger/blob/3fe73a26588876bbf033bf4c4d25c97c2d8564dd/eras/conway/impl/src/Cardano/Ledger/Conway/Tx.hs#L85
            ref_script_cost_multiplier: RationalNumber {
                numerator: 12,
                denominator: 10,
            },
            stake_pool_max_retirement_epoch: 18,
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
            min_pool_cost: 340000000,
            collateral_percentage: 150,
            cost_models: CostModels {
                plutus_v1: Some(vec![
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
                ]),
                plutus_v2: Some(vec![
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
                ]),
                plutus_v3: Some(vec![
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
                ]),
            },
            pool_voting_thresholds: PoolVotingThresholds {
                motion_no_confidence: RationalNumber {
                    numerator: 51,
                    denominator: 100,
                },
                committee_normal: RationalNumber {
                    numerator: 51,
                    denominator: 100,
                },
                committee_no_confidence: RationalNumber {
                    numerator: 51,
                    denominator: 100,
                },
                hard_fork_initiation: RationalNumber {
                    numerator: 51,
                    denominator: 100,
                },
                security_voting_threshold: RationalNumber {
                    numerator: 51,
                    denominator: 100,
                },
            },
            drep_voting_thresholds: DRepVotingThresholds {
                motion_no_confidence: RationalNumber {
                    numerator: 51,
                    denominator: 100,
                },
                committee_normal: RationalNumber {
                    numerator: 67,
                    denominator: 100,
                },
                committee_no_confidence: RationalNumber {
                    numerator: 67,
                    denominator: 100,
                },
                update_constitution: RationalNumber {
                    numerator: 6,
                    denominator: 10,
                },
                hard_fork_initiation: RationalNumber {
                    numerator: 75,
                    denominator: 100,
                },
                pp_network_group: RationalNumber {
                    numerator: 6,
                    denominator: 10,
                },
                pp_economic_group: RationalNumber {
                    numerator: 67,
                    denominator: 100,
                },
                pp_technical_group: RationalNumber {
                    numerator: 67,
                    denominator: 100,
                },
                pp_governance_group: RationalNumber {
                    numerator: 75,
                    denominator: 100,
                },
                treasury_withdrawal: RationalNumber {
                    numerator: 67,
                    denominator: 100,
                },
            },
            min_committee_size: 7,
            max_committee_term_length: 146,
            gov_action_lifetime: 6,
            gov_action_deposit: 100_000_000_000,
            drep_deposit: 500_000_000,
            drep_expiry: 20,
        }
    });

pub static PREVIEW_GLOBAL_PARAMETERS: LazyLock<GlobalParameters> = LazyLock::new(|| {
    let consensus_security_param = 432;
    let active_slot_coeff_inverse = 20;
    let epoch_length_scale_factor = 10;
    let epoch_length =
        active_slot_coeff_inverse * epoch_length_scale_factor * consensus_security_param;
    let stability_window =
        Slot::from((active_slot_coeff_inverse * consensus_security_param * 3) as u64);
    let randomness_stabilization_window =
        (4 * consensus_security_param * active_slot_coeff_inverse) as u64;

    GlobalParameters {
        consensus_security_param,
        epoch_length_scale_factor,
        active_slot_coeff_inverse,
        max_lovelace_supply: 45_000_000_000_000_000,
        slots_per_kes_period: 129_600,
        max_kes_evolution: 62,
        epoch_length,
        stability_window,
        randomness_stabilization_window,
    }
});

// This default is the protocol parameters on Preview as of epoch 646
pub static PREVIEW_INITIAL_PROTOCOL_PARAMETERS: LazyLock<ProtocolParameters> =
    LazyLock::new(|| {
        ProtocolParameters {
            protocol_version: PROTOCOL_VERSION_9,
            min_fee_a: 44,
            min_fee_b: 155381,
            max_block_body_size: 65536,
            max_transaction_size: 16384,
            max_block_header_size: 1100,
            max_tx_ex_units: ExUnits {
                mem: 10_000_000,
                steps: 10_000_000_000,
            },
            max_block_ex_units: ExUnits {
                mem: 50_000_000,
                steps: 40_000_000_000,
            },
            max_value_size: 5000,
            max_collateral_inputs: 3,
            stake_credential_deposit: 2_000_000,
            stake_pool_deposit: 500_000_000,
            lovelace_per_utxo_byte: 4310,
            prices: ExUnitPrices {
                mem_price: RationalNumber {
                    numerator: 577,
                    denominator: 10_000,
                },
                step_price: RationalNumber {
                    numerator: 721,
                    denominator: 10_000_000,
                },
            },
            min_fee_ref_script_lovelace_per_byte: RationalNumber {
                numerator: 15,
                denominator: 1,
            },
            // Hardcoded in the haskell ledger
            // See https://github.com/IntersectMBO/cardano-ledger/blob/3fe73a26588876bbf033bf4c4d25c97c2d8564dd/eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Ledger.hs#L154
            max_ref_script_size_per_tx: 200 * 1024,

            // Hardcoded in the haskell ledger
            // See https://github.com/IntersectMBO/cardano-ledger/blob/3fe73a26588876bbf033bf4c4d25c97c2d8564dd/eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Bbody.hs#L91
            max_ref_script_size_per_block: 1024 * 1024,

            // Hardcoded in the haskell ledger
            // See https://github.com/IntersectMBO/cardano-ledger/blob/3fe73a26588876bbf033bf4c4d25c97c2d8564dd/eras/conway/impl/src/Cardano/Ledger/Conway/Tx.hs#L82
            ref_script_cost_stride: 25600,

            // Hardcoded in the haskell ledger
            // See https://github.com/IntersectMBO/cardano-ledger/blob/3fe73a26588876bbf033bf4c4d25c97c2d8564dd/eras/conway/impl/src/Cardano/Ledger/Conway/Tx.hs#L85
            ref_script_cost_multiplier: RationalNumber {
                numerator: 12,
                denominator: 10,
            },
            stake_pool_max_retirement_epoch: 18,
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
            min_pool_cost: 340000000,
            collateral_percentage: 150,
            cost_models: CostModels {
                plutus_v1: Some(vec![
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
                ]),
                plutus_v2: Some(vec![
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
                ]),
                plutus_v3: Some(vec![
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
                    4, 1293828, 28716, 63, 0, 1, 1006041, 43623, 251, 0, 1,
                ]),
            },
            pool_voting_thresholds: PoolVotingThresholds {
                motion_no_confidence: RationalNumber {
                    numerator: 51,
                    denominator: 100,
                },
                committee_normal: RationalNumber {
                    numerator: 51,
                    denominator: 100,
                },
                committee_no_confidence: RationalNumber {
                    numerator: 51,
                    denominator: 100,
                },
                hard_fork_initiation: RationalNumber {
                    numerator: 51,
                    denominator: 100,
                },
                security_voting_threshold: RationalNumber {
                    numerator: 51,
                    denominator: 100,
                },
            },
            drep_voting_thresholds: DRepVotingThresholds {
                motion_no_confidence: RationalNumber {
                    numerator: 67,
                    denominator: 100,
                },
                committee_normal: RationalNumber {
                    numerator: 67,
                    denominator: 100,
                },
                committee_no_confidence: RationalNumber {
                    numerator: 67,
                    denominator: 100,
                },
                update_constitution: RationalNumber {
                    numerator: 75,
                    denominator: 100,
                },
                hard_fork_initiation: RationalNumber {
                    numerator: 75,
                    denominator: 100,
                },
                pp_network_group: RationalNumber {
                    numerator: 67,
                    denominator: 100,
                },
                pp_economic_group: RationalNumber {
                    numerator: 67,
                    denominator: 100,
                },
                pp_technical_group: RationalNumber {
                    numerator: 67,
                    denominator: 100,
                },
                pp_governance_group: RationalNumber {
                    numerator: 75,
                    denominator: 100,
                },
                treasury_withdrawal: RationalNumber {
                    numerator: 67,
                    denominator: 100,
                },
            },
            min_committee_size: 0,
            max_committee_term_length: 365,
            gov_action_lifetime: 30,
            gov_action_deposit: 100_000_000_000,
            drep_deposit: 500_000_000,
            drep_expiry: 20,
        }
    });

pub static TESTNET_GLOBAL_PARAMETERS: LazyLock<GlobalParameters> = LazyLock::new(|| {
    let consensus_security_param = 432;
    let active_slot_coeff_inverse = 20;
    let epoch_length_scale_factor = 10;
    let epoch_length =
        active_slot_coeff_inverse * epoch_length_scale_factor * consensus_security_param;
    GlobalParameters {
        consensus_security_param,
        epoch_length_scale_factor,
        active_slot_coeff_inverse,
        max_lovelace_supply: 45_000_000_000_000_000,
        slots_per_kes_period: 129_600,
        max_kes_evolution: 62,
        epoch_length,
        stability_window: Slot::from(
            (active_slot_coeff_inverse * consensus_security_param * 2) as u64,
        ),
        randomness_stabilization_window: (4 * consensus_security_param * active_slot_coeff_inverse)
            as u64,
    }
});
