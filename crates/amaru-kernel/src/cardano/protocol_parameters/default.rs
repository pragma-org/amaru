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

use std::sync::LazyLock;

use crate::{
    CostModels, DRepVotingThresholds, ExUnitPrices, ExUnits, PoolVotingThresholds, ProtocolParameters, RationalNumber,
    UnitRationalNumber, PROTOCOL_VERSION_10,
};

// Cost-model vectors are defined in `cost_model.rs` and selected by protocol era.

#[expect(clippy::expect_used)]
pub static MAINNET_DEFAULT_PROTOCOL_PARAMETERS: LazyLock<ProtocolParameters> = LazyLock::new(|| ProtocolParameters {
    protocol_version: PROTOCOL_VERSION_10,
    min_fee_a: 44,
    min_fee_b: 155381,
    max_block_body_size: 90112,
    max_transaction_size: 16384,
    max_block_header_size: 1100,
    max_tx_ex_units: ExUnits { mem: 16_500_000, steps: 10_000_000_000 },
    max_block_ex_units: ExUnits { mem: 72_000_000, steps: 20_000_000_000 },
    max_value_size: 5000,
    max_collateral_inputs: 3,
    stake_credential_deposit: 2_000_000,
    stake_pool_deposit: 500_000_000,
    lovelace_per_utxo_byte: 4310,
    prices: ExUnitPrices {
        mem_price: RationalNumber::new(577, 10_000).expect("577/10_000 is a valid rational number"),
        step_price: RationalNumber::new(721, 10_000_000).expect("721/10_000_000 is a valid rational number"),
    },
    min_fee_ref_script_lovelace_per_byte: RationalNumber::new(15, 1).expect("15/1 is a valid rational number"),
    max_ref_script_size_per_tx: 200 * 1024,
    max_ref_script_size_per_block: 1024 * 1024,
    ref_script_cost_stride: 25600,
    ref_script_cost_multiplier: RationalNumber::new(12, 10).expect("12/10 is a valid rational number"),
    stake_pool_max_retirement_epoch: 18,
    pledge_influence: RationalNumber::new(3, 10).expect("3/10 is a valid rational number"),
    optimal_stake_pools_count: 500,
    #[expect(clippy::expect_used)]
    treasury_expansion_rate: UnitRationalNumber::new(2, 10).expect("2 <= 10"),
    #[expect(clippy::expect_used)]
    monetary_expansion_rate: UnitRationalNumber::new(3, 1_000).expect("3 <= 1000"),
    min_pool_cost: 170000000,
    collateral_percentage: 150,
    cost_models: CostModels::default_for(PROTOCOL_VERSION_10),
    pool_voting_thresholds: PoolVotingThresholds {
        motion_no_confidence: UnitRationalNumber::new(51, 100).expect("51/100 is a valid rational number"),
        committee_normal: UnitRationalNumber::new(51, 100).expect("51/100 is a valid rational number"),
        committee_no_confidence: UnitRationalNumber::new(51, 100).expect("51/100 is a valid rational number"),
        hard_fork_initiation: UnitRationalNumber::new(51, 100).expect("51/100 is a valid rational number"),
        security_voting_threshold: UnitRationalNumber::new(51, 100).expect("51/100 is a valid rational number"),
    },
    drep_voting_thresholds: DRepVotingThresholds {
        motion_no_confidence: UnitRationalNumber::new(67, 100).expect("67/100 is a valid rational number"),
        committee_normal: UnitRationalNumber::new(67, 100).expect("67/100 is a valid rational number"),
        committee_no_confidence: UnitRationalNumber::new(60, 100).expect("60/100 is a valid rational number"),
        update_constitution: UnitRationalNumber::new(75, 100).expect("75/100 is a valid rational number"),
        hard_fork_initiation: UnitRationalNumber::new(60, 100).expect("60/100 is a valid rational number"),
        pp_network_group: UnitRationalNumber::new(67, 100).expect("67/100 is a valid rational number"),
        pp_economic_group: UnitRationalNumber::new(67, 100).expect("67/100 is a valid rational number"),
        pp_technical_group: UnitRationalNumber::new(67, 100).expect("67/100 is a valid rational number"),
        pp_governance_group: UnitRationalNumber::new(75, 100).expect("75/100 is a valid rational number"),
        treasury_withdrawal: UnitRationalNumber::new(67, 100).expect("67/100 is a valid rational number"),
    },
    min_committee_size: 7,
    max_committee_term_length: 146,
    gov_action_lifetime: 6,
    gov_action_deposit: 100_000_000_000,
    drep_deposit: 500_000_000,
    drep_expiry: 20,
});

// This default is the protocol parameters on Preprod as of epoch 197
#[expect(clippy::expect_used)]
pub static PREPROD_DEFAULT_PROTOCOL_PARAMETERS: LazyLock<ProtocolParameters> = LazyLock::new(|| {
    ProtocolParameters {
        protocol_version: PROTOCOL_VERSION_10,
        min_fee_a: 44,
        min_fee_b: 155381,
        max_block_body_size: 90112,
        max_transaction_size: 16384,
        max_block_header_size: 1100,
        max_tx_ex_units: ExUnits { mem: 14_000_000, steps: 10_000_000_000 },
        max_block_ex_units: ExUnits { mem: 62_000_000, steps: 20_000_000_000 },
        max_value_size: 5000,
        max_collateral_inputs: 3,
        stake_credential_deposit: 2_000_000,
        stake_pool_deposit: 500_000_000,
        lovelace_per_utxo_byte: 4310,
        prices: ExUnitPrices {
            mem_price: RationalNumber::new(577, 10_000).expect("577/10_000 is a valid rational number"),
            step_price: RationalNumber::new(721, 10_000_000).expect("721/10_000_000 is a valid rational number"),
        },
        min_fee_ref_script_lovelace_per_byte: RationalNumber::new(15, 1).expect("15/1 is a valid rational number"),
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
        ref_script_cost_multiplier: RationalNumber::new(12, 10).expect("12/10 is a valid rational number"),
        stake_pool_max_retirement_epoch: 18,
        pledge_influence: RationalNumber::new(3, 10).expect("3/10 is a valid rational number"),
        optimal_stake_pools_count: 500,
        #[expect(clippy::expect_used)]
        treasury_expansion_rate: UnitRationalNumber::new(2, 10).expect("2 <= 10"),
        #[expect(clippy::expect_used)]
        monetary_expansion_rate: UnitRationalNumber::new(3, 1_000).expect("3 <= 1000"),
        min_pool_cost: 340000000,
        collateral_percentage: 150,
        cost_models: CostModels::default_for(PROTOCOL_VERSION_10),
        pool_voting_thresholds: PoolVotingThresholds {
            motion_no_confidence: UnitRationalNumber::new(51, 100).expect("51/100 is a valid rational number"),
            committee_normal: UnitRationalNumber::new(51, 100).expect("51/100 is a valid rational number"),
            committee_no_confidence: UnitRationalNumber::new(51, 100).expect("51/100 is a valid rational number"),
            hard_fork_initiation: UnitRationalNumber::new(51, 100).expect("51/100 is a valid rational number"),
            security_voting_threshold: UnitRationalNumber::new(51, 100).expect("51/100 is a valid rational number"),
        },
        drep_voting_thresholds: DRepVotingThresholds {
            motion_no_confidence: UnitRationalNumber::new(51, 100).expect("51/100 is a valid rational number"),
            committee_normal: UnitRationalNumber::new(67, 100).expect("67/100 is a valid rational number"),
            committee_no_confidence: UnitRationalNumber::new(67, 100).expect("67/100 is a valid rational number"),
            update_constitution: UnitRationalNumber::new(6, 10).expect("6/10 is a valid rational number"),
            hard_fork_initiation: UnitRationalNumber::new(75, 100).expect("75/100 is a valid rational number"),
            pp_network_group: UnitRationalNumber::new(6, 10).expect("6/10 is a valid rational number"),
            pp_economic_group: UnitRationalNumber::new(67, 100).expect("67/100 is a valid rational number"),
            pp_technical_group: UnitRationalNumber::new(67, 100).expect("67/100 is a valid rational number"),
            pp_governance_group: UnitRationalNumber::new(75, 100).expect("75/100 is a valid rational number"),
            treasury_withdrawal: UnitRationalNumber::new(67, 100).expect("67/100 is a valid rational number"),
        },
        min_committee_size: 7,
        max_committee_term_length: 146,
        gov_action_lifetime: 6,
        gov_action_deposit: 100_000_000_000,
        drep_deposit: 500_000_000,
        drep_expiry: 20,
    }
});

// This default is the protocol parameters on Preview as of epoch 646
#[expect(clippy::expect_used)]
pub static PREVIEW_DEFAULT_PROTOCOL_PARAMETERS: LazyLock<ProtocolParameters> = LazyLock::new(|| {
    ProtocolParameters {
        protocol_version: PROTOCOL_VERSION_10,
        min_fee_a: 44,
        min_fee_b: 155381,
        max_block_body_size: 65536,
        max_transaction_size: 16384,
        max_block_header_size: 1100,
        max_tx_ex_units: ExUnits { mem: 10_000_000, steps: 10_000_000_000 },
        max_block_ex_units: ExUnits { mem: 50_000_000, steps: 40_000_000_000 },
        max_value_size: 5000,
        max_collateral_inputs: 3,
        stake_credential_deposit: 2_000_000,
        stake_pool_deposit: 500_000_000,
        lovelace_per_utxo_byte: 4310,
        prices: ExUnitPrices {
            mem_price: RationalNumber::new(577, 10_000).expect("577/10000 is a valid rational number"),
            step_price: RationalNumber::new(721, 10_000_000).expect("721/10000000 is a valid rational number"),
        },
        min_fee_ref_script_lovelace_per_byte: RationalNumber::new(15, 1).expect("15/1 is a valid rational number"),
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
        ref_script_cost_multiplier: RationalNumber::new(12, 10).expect("12/10 is a valid rational number"),
        stake_pool_max_retirement_epoch: 18,
        pledge_influence: RationalNumber::new(3, 10).expect("3/10 is a valid rational number"),
        optimal_stake_pools_count: 500,
        #[expect(clippy::expect_used)]
        treasury_expansion_rate: UnitRationalNumber::new(2, 10).expect("2 <= 10"),
        #[expect(clippy::expect_used)]
        monetary_expansion_rate: UnitRationalNumber::new(3, 1_000).expect("3 <= 1000"),
        min_pool_cost: 340000000,
        collateral_percentage: 150,
        cost_models: CostModels::default_for(PROTOCOL_VERSION_10),
        pool_voting_thresholds: PoolVotingThresholds {
            motion_no_confidence: UnitRationalNumber::new(51, 100).expect("51/100 is a valid rational number"),
            committee_normal: UnitRationalNumber::new(51, 100).expect("51/100 is a valid rational number"),
            committee_no_confidence: UnitRationalNumber::new(51, 100).expect("51/100 is a valid rational number"),
            hard_fork_initiation: UnitRationalNumber::new(51, 100).expect("51/100 is a valid rational number"),
            security_voting_threshold: UnitRationalNumber::new(51, 100).expect("51/100 is a valid rational number"),
        },
        drep_voting_thresholds: DRepVotingThresholds {
            motion_no_confidence: UnitRationalNumber::new(67, 100).expect("67/100 is a valid rational number"),
            committee_normal: UnitRationalNumber::new(67, 100).expect("67/100 is a valid rational number"),
            committee_no_confidence: UnitRationalNumber::new(67, 100).expect("67/100 is a valid rational number"),
            update_constitution: UnitRationalNumber::new(75, 100).expect("75/100 is a valid rational number"),
            hard_fork_initiation: UnitRationalNumber::new(75, 100).expect("75/100 is a valid rational number"),
            pp_network_group: UnitRationalNumber::new(67, 100).expect("67/100 is a valid rational number"),
            pp_economic_group: UnitRationalNumber::new(67, 100).expect("67/100 is a valid rational number"),
            pp_technical_group: UnitRationalNumber::new(67, 100).expect("67/100 is a valid rational number"),
            pp_governance_group: UnitRationalNumber::new(75, 100).expect("75/100 is a valid rational number"),
            treasury_withdrawal: UnitRationalNumber::new(67, 100).expect("67/100 is a valid rational number"),
        },
        min_committee_size: 0,
        max_committee_term_length: 365,
        gov_action_lifetime: 30,
        gov_action_deposit: 100_000_000_000,
        drep_deposit: 500_000_000,
        drep_expiry: 20,
    }
});
