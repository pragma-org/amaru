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

use amaru_kernel::{cbor, Epoch, Proposal, ProposalId, ProposalPointer};
use iter_borrow::IterBorrow;

pub const EVENT_TARGET: &str = "amaru::ledger::store::proposals";

/// Iterator used to browse rows from the Accounts column. Meant to be referenced using qualified imports.
pub type Iter<'a, 'b> = IterBorrow<'a, 'b, Key, Option<Row>>;

pub type Key = ProposalId;

pub type Value = Row;

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub proposed_in: ProposalPointer,
    pub valid_until: Epoch,
    pub proposal: Proposal,
}

impl Row {
    #[allow(clippy::panic)]
    pub fn unsafe_decode(bytes: Vec<u8>) -> Self {
        cbor::decode(&bytes).unwrap_or_else(|e| {
            panic!(
                "unable to decode account from CBOR ({}): {e:?}",
                hex::encode(&bytes)
            )
        })
    }
}

impl<C> cbor::encode::Encode<C> for Row {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(3)?;
        e.encode_with(self.proposed_in, ctx)?;
        e.encode_with(self.valid_until, ctx)?;
        e.encode_with(&self.proposal, ctx)?;
        Ok(())
    }
}

impl<'a, C> cbor::decode::Decode<'a, C> for Row {
    fn decode(d: &mut cbor::Decoder<'a>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        Ok(Row {
            proposed_in: d.decode_with(ctx)?,
            valid_until: d.decode_with(ctx)?,
            proposal: d.decode_with(ctx)?,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::store::{
        accounts::test::any_stake_credential,
        columns::dreps::tests::{any_anchor, any_transaction_pointer},
    };
    use amaru_kernel::{
        new_stake_address, prop_cbor_roundtrip, Bytes, Constitution, CostModel, CostModels,
        DRepVotingThresholds, ExUnitPrices, ExUnits, GovAction, Hash, KeyValuePairs, Lovelace,
        Network, Nullable, PoolVotingThresholds, ProposalId, ProtocolParamUpdate, ProtocolVersion,
        RewardAccount, ScriptHash, Set, StakeCredential, StakePayload, UnitInterval,
    };
    use proptest::{option, prelude::*};

    // Causes a StackOverflow on Windows, somehow...
    #[cfg(not(target_os = "windows"))]
    prop_cbor_roundtrip!(prop_cbor_roundtrip_row, Row, any_row());

    prop_cbor_roundtrip!(prop_cbor_roundtrip_key, Key, any_proposal_id());

    prop_compose! {
        pub(crate) fn any_proposal_id()(
            transaction_id in any::<[u8; 32]>(),
            action_index in any::<u32>(),
        ) -> ProposalId {
            ProposalId {
                transaction_id: Hash::new(transaction_id),
                action_index,
            }
        }
    }

    prop_compose! {
        pub(crate) fn any_unit_interval()(
            numerator in any::<u64>(),
            denominator in 1..u64::MAX,
        ) -> UnitInterval {
            UnitInterval {
                numerator,
                denominator,
            }
        }
    }

    prop_compose! {
        pub(crate) fn any_drep_voting_thresholds()(
            motion_no_confidence in any_unit_interval(),
            committee_normal in any_unit_interval(),
            committee_no_confidence in any_unit_interval(),
            update_constitution in any_unit_interval(),
            hard_fork_initiation in any_unit_interval(),
            pp_network_group in any_unit_interval(),
            pp_economic_group in any_unit_interval(),
            pp_technical_group in any_unit_interval(),
            pp_governance_group in any_unit_interval(),
            treasury_withdrawal in any_unit_interval(),
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
        pub(crate) fn any_pool_voting_thresholds()(
            motion_no_confidence in any_unit_interval(),
            committee_normal in any_unit_interval(),
            committee_no_confidence in any_unit_interval(),
            hard_fork_initiation in any_unit_interval(),
            security_voting_threshold in any_unit_interval(),
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
        pub(crate) fn any_cost_model()(
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
        pub(crate) fn any_cost_models()(
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
        pub(crate) fn any_ex_unit_prices()(
            mem_price in any_unit_interval(),
            step_price in any_unit_interval(),
        ) -> ExUnitPrices {
            ExUnitPrices {
                mem_price,
                step_price,
            }

        }
    }

    prop_compose! {
        pub(crate) fn any_ex_units()(
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
        pub(crate) fn any_protocol_params_update()(
            minfee_a in option::of(any::<u64>()),
            minfee_b in option::of(any::<u64>()),
            max_block_body_size in option::of(any::<u64>()),
            max_transaction_size in option::of(any::<u64>()),
            max_block_header_size in option::of(any::<u64>()),
            key_deposit in option::of(any::<Lovelace>()),
            pool_deposit in option::of(any::<Lovelace>()),
            maximum_epoch in option::of(any::<Epoch>()),
            desired_number_of_stake_pools in option::of(any::<u64>()),
            pool_pledge_influence in option::of(any_unit_interval()),
            expansion_rate in option::of(any_unit_interval()),
            treasury_growth_rate in option::of(any_unit_interval()),
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
            committee_term_limit in option::of(any::<Epoch>()),
            governance_action_validity_period in option::of(any::<Epoch>()),
            governance_action_deposit in option::of(any::<Lovelace>()),
            drep_deposit in option::of(any::<Lovelace>()),
            drep_inactivity_period in option::of(any::<Epoch>()),
            minfee_refscript_cost_per_byte in option::of(any_unit_interval()),
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
                minfee_refscript_cost_per_byte
            }
        }
    }

    prop_compose! {
        pub(crate) fn any_protocol_version()(
            major in any::<u64>(),
            minor in any::<u64>(),
        ) -> ProtocolVersion {
            (major, minor)
        }
    }

    prop_compose! {
        pub(crate) fn any_withdrawal()(
            reward_account in any_reward_account(),
            amount in any::<Lovelace>(),
        ) -> (RewardAccount, Lovelace) {
            (reward_account, amount)
        }
    }

    prop_compose! {
        pub(crate) fn any_guardrails_script()(
            script in option::of(any::<[u8; 28]>()),
        ) -> Nullable<ScriptHash> {
            Nullable::from(script.map(Hash::new))
        }
    }

    prop_compose! {
        pub(crate) fn any_constitution()(
            anchor in any_anchor(),
            guardrail_script in any_guardrails_script(),
        ) -> Constitution {
            Constitution {
                anchor,
                guardrail_script,
            }
        }
    }

    pub(crate) fn any_gov_action() -> impl Strategy<Value = GovAction> {
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
                withdrawals in prop::collection::vec(any_withdrawal(), 0..3),
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
                epoch in any::<Epoch>(),
            ) -> (StakeCredential, Epoch) {
                (credential, epoch)
            }
        }

        prop_compose! {
            fn any_committee_update()(
                parent_proposal_id in any_parent_proposal_id(),
                to_remove in prop::collection::btree_set(any_stake_credential(), 0..3),
                to_add in prop::collection::vec(any_committee_registration(), 0..3),
                is_definite in any::<bool>(),
                quorum in any_unit_interval(),
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
        pub(crate) fn any_network()(
            is_testnet in any::<bool>(),
        ) -> Network {
            if is_testnet {
                Network::Testnet
            } else {
                Network::Mainnet
            }
        }

    }

    prop_compose! {
        pub(crate) fn any_reward_account()(
            network in any_network(),
            credential in any::<[u8; 28]>(),
            is_script in any::<bool>(),
        ) -> RewardAccount {
            let payload = if is_script {
                StakePayload::Script
            } else {
                StakePayload::Stake
            }(Hash::new(credential));

            Bytes::from(new_stake_address(network, payload).to_vec())
        }
    }

    prop_compose! {
        pub(crate) fn any_proposal()(
            deposit in any::<Lovelace>(),
            reward_account in any_reward_account(),
            gov_action in any_gov_action(),
            anchor in any_anchor(),
        ) -> Proposal {
            Proposal {
                deposit,
                reward_account,
                gov_action,
                anchor,
            }
        }
    }

    prop_compose! {
        pub(crate) fn any_proposal_pointer()(
            transaction in any_transaction_pointer(),
            proposal_index in any::<usize>(),
        ) -> ProposalPointer {
            ProposalPointer {
                transaction,
                proposal_index,
            }
        }
    }

    prop_compose! {
        pub(crate) fn any_row()(
            proposed_in in any_proposal_pointer(),
            valid_until in any::<Epoch>(),
            proposal in any_proposal(),
        ) -> Row {
            Row {
                proposed_in,
                valid_until,
                proposal,
            }
        }

    }
}
