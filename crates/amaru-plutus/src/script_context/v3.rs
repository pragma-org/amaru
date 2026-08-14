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

use std::collections::BTreeMap;

use amaru_kernel::{
    Address, Certificate, Constitution, CostModels, DRep, DRepVotingThresholds, ExUnitPrices, ExUnits,
    GovernanceAction, MemoizedTransactionOutput, PlutusData, PoolParams, PoolVotingThresholds, Proposal, ProposalId,
    ProtocolParamUpdate, RationalNumber, StakeAddress, StakeCredential, StakePayload, TransactionInput, Vote, Voter,
};
use num::Integer;

use crate::{
    PlutusDataError, ToPlutusData, constr, constr_v3,
    script_context::{
        OutputReference, PlutusDatums, PlutusMint, PlutusStakeAddress, PlutusVotes, PlutusWithdrawals, ScriptContext,
        ScriptInfo, ScriptPurpose, TxInfo,
    },
};

impl ToPlutusData<3> for OutputReference<'_> {
    /// Serialize an `OutputReference` as PlutusData for PlutusV3.
    ///
    /// # Errors
    /// If the UTxO is locked at a bootstrap address, this will return a `PlutusDataError`.
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        if let Address::Byron(_) = self.output.address {
            return Err(PlutusDataError::unsupported_version("byron address included in OutputReference", 3));
        }

        constr_v3!(0, [self.input, self.output])
    }
}

impl ToPlutusData<3> for ScriptContext<'_> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        constr_v3!(0, [self.tx_info, self.redeemer_data, self.script_purpose.to_script_info(self.datum)])
    }
}

impl ToPlutusData<3> for TxInfo<'_> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        constr_v3!(
            0,
            [
                self.inputs,
                self.reference_inputs,
                self.outputs,
                self.fee,
                self.mint,
                self.certificates,
                self.withdrawals,
                self.valid_range,
                self.signatories,
                self.redeemers,
                self.data,
                self.id,
                self.votes,
                self.proposal_procedures,
                self.current_treasury_amount,
                self.treasury_donation,
            ]
        )
    }
}

impl ToPlutusData<3> for ScriptPurpose<'_> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self {
            ScriptPurpose::Minting(policy_id) => constr_v3!(0, [policy_id]),
            ScriptPurpose::Spending(out_ref, _) => constr_v3!(1, [out_ref]),
            ScriptPurpose::Rewarding(stake_credential) => constr_v3!(2, [stake_credential]),
            ScriptPurpose::Certifying(ix, certificate) => constr_v3!(3, [ix, certificate]),
            ScriptPurpose::Voting(voter) => constr_v3!(4, [voter]),
            ScriptPurpose::Proposing(ix, procedure) => constr_v3!(5, [ix, procedure]),
        }
    }
}

impl ToPlutusData<3> for ScriptInfo<'_, Option<&'_ PlutusData>> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self {
            ScriptInfo::Minting(policy_id) => constr_v3!(0, [policy_id]),
            ScriptInfo::Spending(out_ref, datum) => constr_v3!(1, [out_ref, datum]),
            ScriptInfo::Rewarding(stake_credential) => constr_v3!(2, [stake_credential]),
            ScriptInfo::Certifying(ix, dcert) => constr_v3!(3, [ix, dcert]),
            ScriptInfo::Voting(voter) => constr_v3!(4, [voter]),
            ScriptInfo::Proposing(ix, procedure) => constr_v3!(5, [ix, procedure]),
        }
    }
}

impl ToPlutusData<3> for TransactionInput {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        constr_v3!(0, [self.transaction_id, self.index])
    }
}

impl ToPlutusData<3> for MemoizedTransactionOutput {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        constr_v3!(0, [self.address, self.value.as_ref(), self.datum, self.script])
    }
}

impl ToPlutusData<3> for DRep {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self {
            DRep::Key(hash) => constr_v3!(0, [StakeCredential::AddrKeyhash(*hash)]),
            DRep::Script(hash) => constr_v3!(0, [StakeCredential::ScriptHash(*hash)]),
            DRep::Abstain => constr!(1),
            DRep::NoConfidence => constr!(2),
        }
    }
}

impl ToPlutusData<3> for Certificate {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self {
            Certificate::StakeRegistration(stake_credential) => {
                constr_v3!(0, [stake_credential, None::<PlutusData>])
            }
            Certificate::Reg(stake_credential, coin) => constr_v3!(0, [stake_credential, Some(coin)]),
            Certificate::StakeDeregistration(stake_credential) => {
                constr_v3!(1, [stake_credential, None::<PlutusData>])
            }
            Certificate::UnReg(stake_credential, coin) => constr_v3!(1, [stake_credential, Some(coin)]),
            Certificate::StakeDelegation(stake_credential, pool_id) => {
                constr_v3!(2, [stake_credential, constr_v3!(0, [pool_id])?])
            }
            Certificate::VoteDeleg(stake_credential, drep) => {
                constr_v3!(2, [stake_credential, constr_v3!(1, [drep])?])
            }
            Certificate::StakeVoteDeleg(stake_credential, pool_id, drep) => {
                constr_v3!(2, [stake_credential, constr_v3!(2, [pool_id, drep])?])
            }
            Certificate::StakeRegDeleg(stake_credential, pool_id, deposit) => {
                constr_v3!(3, [stake_credential, constr_v3!(0, [pool_id])?, deposit])
            }
            Certificate::VoteRegDeleg(stake_credential, drep, deposit) => {
                constr_v3!(3, [stake_credential, constr_v3!(1, [drep])?, deposit])
            }
            Certificate::StakeVoteRegDeleg(stake_credential, pool_id, drep, deposit) => {
                constr_v3!(3, [stake_credential, constr_v3!(2, [pool_id, drep])?, deposit])
            }
            Certificate::RegDRepCert(drep_credential, deposit, _anchor) => {
                constr_v3!(4, [drep_credential, deposit])
            }
            Certificate::UpdateDRepCert(drep_credential, _anchor) => {
                constr_v3!(5, [drep_credential])
            }
            Certificate::UnRegDRepCert(drep_credential, deposit) => {
                constr_v3!(6, [drep_credential, deposit])
            }
            Certificate::PoolRegistration(params) => {
                let PoolParams {
                    id,
                    vrf,
                    pledge: _,
                    cost: _,
                    margin: _,
                    reward_account: _,
                    owners: _,
                    relays: _,
                    metadata: _,
                } = params.as_ref();
                constr_v3!(7, [id, vrf])
            }
            Certificate::PoolRetirement(pool_keyhash, epoch) => {
                constr_v3!(8, [pool_keyhash, epoch])
            }
            Certificate::AuthCommitteeHot(cold_credential, hot_credential) => {
                constr_v3!(9, [cold_credential, hot_credential])
            }
            Certificate::ResignCommitteeCold(cold_credential, _anchor) => {
                constr_v3!(10, [cold_credential])
            }
        }
    }
}

impl ToPlutusData<3> for Voter {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self {
            Voter::ConstitutionalCommitteeScript(hash) => {
                constr_v3!(0, [StakeCredential::ScriptHash(*hash)])
            }
            Voter::ConstitutionalCommitteeKey(hash) => {
                constr_v3!(0, [StakeCredential::AddrKeyhash(*hash)])
            }
            Voter::DRepScript(hash) => {
                constr_v3!(1, [StakeCredential::ScriptHash(*hash)])
            }
            Voter::DRepKey(hash) => {
                constr_v3!(1, [StakeCredential::AddrKeyhash(*hash)])
            }
            Voter::StakePoolKey(hash) => constr_v3!(2, [hash]),
        }
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
impl ToPlutusData<3> for Proposal {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        constr_v3!(0, [self.deposit, Address::from_bytes(&self.reward_account).unwrap(), self.gov_action])
    }
}

#[allow(clippy::expect_used)]
impl ToPlutusData<3> for GovernanceAction {
    /// Serializes a `GovernanceAction` to PlutusData for PlutusV3.
    ///
    ///
    /// # Errors
    ///
    /// This will only return an error if
    /// a treasury withdrawal is to an invalid reward address.
    /// This can only happen if the transaction is poorly constructed,
    /// in which case it will fail phase-one validation.
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self {
            GovernanceAction::ParameterChange(previous_action, params, guardrail) => {
                constr_v3!(0, [previous_action, params.as_ref(), guardrail])
            }
            GovernanceAction::HardForkInitiation(previous_action, version) => {
                constr_v3!(1, [previous_action, version])
            }
            GovernanceAction::TreasuryWithdrawals(withdrawals, guardrail) => {
                let withdrawals = withdrawals
                    .iter()
                    .map(|(reward_account, amount)| {
                        let reward_address =
                            if let Some(Address::Stake(reward_address)) = Address::from_bytes(reward_account) {
                                Ok(reward_address)
                            } else {
                                Err(PlutusDataError::Custom("invalid stake address in treasury withdrawal?".into()))
                            }?;

                        Ok((reward_address, *amount))
                    })
                    .collect::<Result<BTreeMap<_, _>, _>>()?;

                constr_v3!(2, [withdrawals, guardrail])
            }
            GovernanceAction::NoConfidence(previous_action) => {
                constr_v3!(3, [previous_action])
            }
            GovernanceAction::UpdateCommittee(previous_action, removed, added, quorum) => {
                let quorum = governance_action_ratio(quorum)?;
                constr_v3!(4, [previous_action, removed, added, quorum])
            }
            GovernanceAction::NewConstitution(previous_action, constitution) => {
                constr_v3!(5, [previous_action, constitution])
            }
            GovernanceAction::Information => constr!(6),
        }
    }
}

impl ToPlutusData<3> for Constitution {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        constr_v3!(0, [self.guardrail_script])
    }
}

impl ToPlutusData<3> for ProposalId {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        constr_v3!(0, [self.transaction_id, self.action_index])
    }
}

impl ToPlutusData<3> for ProtocolParamUpdate {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        let mut pparams = Vec::with_capacity(30);

        let mut push = |ix: usize, p: Result<PlutusData, PlutusDataError>| -> Result<(), PlutusDataError> {
            pparams.push((<usize as ToPlutusData<3>>::to_plutus_data(&ix)?, p?));
            Ok(())
        };

        if let Some(p) = self.minfee_a {
            push(0, <u64 as ToPlutusData<3>>::to_plutus_data(&p))?;
        }

        if let Some(p) = self.minfee_b {
            push(1, <u64 as ToPlutusData<3>>::to_plutus_data(&p))?;
        }

        if let Some(p) = self.max_block_body_size {
            push(2, <u64 as ToPlutusData<3>>::to_plutus_data(&p))?;
        }

        if let Some(p) = self.max_transaction_size {
            push(3, <u64 as ToPlutusData<3>>::to_plutus_data(&p))?;
        }

        if let Some(p) = self.max_block_header_size {
            push(4, <u64 as ToPlutusData<3>>::to_plutus_data(&p))?;
        }

        if let Some(p) = self.key_deposit {
            push(5, <u64 as ToPlutusData<3>>::to_plutus_data(&p))?;
        }

        if let Some(p) = self.pool_deposit {
            push(6, <u64 as ToPlutusData<3>>::to_plutus_data(&p))?;
        }

        if let Some(p) = self.maximum_epoch {
            push(7, <u64 as ToPlutusData<3>>::to_plutus_data(&p))?;
        }

        if let Some(p) = self.desired_number_of_stake_pools {
            push(8, <u64 as ToPlutusData<3>>::to_plutus_data(&p))?;
        }

        if let Some(ref p) = self.pool_pledge_influence {
            push(9, protocol_parameter_ratio(p))?;
        }

        if let Some(ref p) = self.expansion_rate {
            push(10, protocol_parameter_ratio(p))?;
        }

        if let Some(ref p) = self.treasury_growth_rate {
            push(11, protocol_parameter_ratio(p))?;
        }

        if let Some(p) = self.min_pool_cost {
            push(16, <u64 as ToPlutusData<3>>::to_plutus_data(&p))?;
        }

        if let Some(p) = self.ada_per_utxo_byte {
            push(17, <u64 as ToPlutusData<3>>::to_plutus_data(&p))?;
        }

        // TODO: this is from Aiken, need to implement this
        if let Some(cost_models) = &self.cost_models_for_script_languages {
            push(18, <CostModels as ToPlutusData<3>>::to_plutus_data(cost_models))?;
        }

        if let Some(ref p) = self.execution_costs {
            push(19, p.to_plutus_data())?;
        }

        if let Some(p) = self.max_tx_ex_units {
            push(20, p.to_plutus_data())?;
        }

        if let Some(p) = self.max_block_ex_units {
            push(21, p.to_plutus_data())?;
        }

        if let Some(p) = self.max_value_size {
            push(22, <u64 as ToPlutusData<3>>::to_plutus_data(&p))?;
        }

        if let Some(p) = self.collateral_percentage {
            push(23, <u64 as ToPlutusData<3>>::to_plutus_data(&p))?;
        }

        if let Some(p) = self.max_collateral_inputs {
            push(24, <u64 as ToPlutusData<3>>::to_plutus_data(&p))?;
        }

        if let Some(ref p) = self.pool_voting_thresholds {
            push(25, p.to_plutus_data())?;
        }

        if let Some(ref p) = self.drep_voting_thresholds {
            push(26, p.to_plutus_data())?;
        }

        if let Some(p) = self.min_committee_size {
            push(27, <u64 as ToPlutusData<3>>::to_plutus_data(&p))?;
        }

        if let Some(p) = self.committee_term_limit {
            push(28, <u64 as ToPlutusData<3>>::to_plutus_data(&p))?;
        }

        if let Some(p) = self.governance_action_validity_period {
            push(29, <u64 as ToPlutusData<3>>::to_plutus_data(&p))?;
        }

        if let Some(p) = self.governance_action_deposit {
            push(30, <u64 as ToPlutusData<3>>::to_plutus_data(&p))?;
        }

        if let Some(p) = self.drep_deposit {
            push(31, <u64 as ToPlutusData<3>>::to_plutus_data(&p))?;
        }

        if let Some(p) = self.drep_inactivity_period {
            push(32, <u64 as ToPlutusData<3>>::to_plutus_data(&p))?;
        }

        if let Some(ref p) = self.minfee_refscript_cost_per_byte {
            push(33, protocol_parameter_ratio(p))?;
        }

        Ok(PlutusData::Map(pparams))
    }
}

impl ToPlutusData<3> for CostModels {
    /// The ledger flattens the cost models into a map from language identifier to a cost model:
    /// 0 (V1), 1 (V2), 2 (V3), in ascending order.
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        let CostModels { plutus_v1, plutus_v2, plutus_v3 } = self;
        let mut models = BTreeMap::new();
        for (language, model) in [(0u64, plutus_v1), (1, plutus_v2), (2, plutus_v3)] {
            if let Some(costs) = model {
                models.insert(language, costs.clone());
            }
        }
        <BTreeMap<_, _> as ToPlutusData<3>>::to_plutus_data(&models)
    }
}

fn normalized_ratio(ratio: &RationalNumber) -> (u64, u64) {
    let gcd = ratio.numerator.gcd(&ratio.denominator);
    (ratio.numerator / gcd, ratio.denominator / gcd)
}

fn governance_action_ratio(ratio: &RationalNumber) -> Result<PlutusData, PlutusDataError> {
    let (numerator, denominator) = normalized_ratio(ratio);
    constr_v3!(0, [numerator, denominator])
}

fn protocol_parameter_ratio(ratio: &RationalNumber) -> Result<PlutusData, PlutusDataError> {
    let (numerator, denominator) = normalized_ratio(ratio);
    <Vec<_> as ToPlutusData<3>>::to_plutus_data(&vec![numerator, denominator])
}

impl ToPlutusData<3> for ExUnitPrices {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        <Vec<_> as ToPlutusData<3>>::to_plutus_data(&vec![
            protocol_parameter_ratio(&self.mem_price)?,
            protocol_parameter_ratio(&self.step_price)?,
        ])
    }
}

impl ToPlutusData<3> for PoolVotingThresholds {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        <Vec<_> as ToPlutusData<3>>::to_plutus_data(&vec![
            protocol_parameter_ratio(&self.motion_no_confidence)?,
            protocol_parameter_ratio(&self.committee_normal)?,
            protocol_parameter_ratio(&self.committee_no_confidence)?,
            protocol_parameter_ratio(&self.hard_fork_initiation)?,
            protocol_parameter_ratio(&self.security_voting_threshold)?,
        ])
    }
}

impl ToPlutusData<3> for DRepVotingThresholds {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        <Vec<_> as ToPlutusData<3>>::to_plutus_data(&vec![
            protocol_parameter_ratio(&self.motion_no_confidence)?,
            protocol_parameter_ratio(&self.committee_normal)?,
            protocol_parameter_ratio(&self.committee_no_confidence)?,
            protocol_parameter_ratio(&self.update_constitution)?,
            protocol_parameter_ratio(&self.hard_fork_initiation)?,
            protocol_parameter_ratio(&self.pp_network_group)?,
            protocol_parameter_ratio(&self.pp_economic_group)?,
            protocol_parameter_ratio(&self.pp_technical_group)?,
            protocol_parameter_ratio(&self.pp_governance_group)?,
            protocol_parameter_ratio(&self.treasury_withdrawal)?,
        ])
    }
}

impl ToPlutusData<3> for ExUnits {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        <Vec<_> as ToPlutusData<3>>::to_plutus_data(&vec![&self.mem, &self.steps])
    }
}

impl ToPlutusData<3> for Vote {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self {
            Vote::No => constr!(0),
            Vote::Yes => constr!(1),
            Vote::Abstain => constr!(2),
        }
    }
}

impl ToPlutusData<3> for PlutusMint<'_> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        <BTreeMap<_, _> as ToPlutusData<3>>::to_plutus_data(&self.0)
    }
}

impl ToPlutusData<3> for PlutusWithdrawals {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        <BTreeMap<_, _> as ToPlutusData<3>>::to_plutus_data(&self.iter().collect::<BTreeMap<_, _>>())
    }
}

impl ToPlutusData<3> for PlutusDatums<'_> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        <BTreeMap<_, _> as ToPlutusData<3>>::to_plutus_data(&self.0)
    }
}

impl ToPlutusData<3> for PlutusVotes<'_> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        self.0.to_plutus_data()
    }
}

impl ToPlutusData<3> for StakeAddress {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self.payload() {
            StakePayload::Key(keyhash) => constr_v3!(0, [keyhash]),
            StakePayload::Script(script_hash) => constr_v3!(1, [script_hash]),
        }
    }
}

impl ToPlutusData<3> for PlutusStakeAddress {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        <amaru_kernel::StakeAddress as ToPlutusData<3>>::to_plutus_data(self.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Deref;

    use amaru_kernel::{KeyValuePairs, PREPROD_ERA_HISTORY, PREPROD_GLOBAL_PARAMETERS, Transaction, cbor, to_cbor};
    use test_case::test_case;

    use super::{
        super::test_vectors::{self, TestVector},
        *,
    };

    macro_rules! fixture {
        ($title:literal) => {
            test_vectors::get_test_vector($title, 3)
        };
    }

    #[test_case(fixture!("simple_send"); "simple send")]
    #[test_case(fixture!("simple_spend_no_datum"); "simple spend no datum")]
    #[test_case(fixture!("mint"); "mint")]
    #[test_case(fixture!("certificates_v10"); "certificates (protocol ver 10")]
    #[test_case(fixture!("duplicate_redeemers_last_wins"); "duplicate redeemers last wins")]
    fn test_plutus_v3(test_vector: &TestVector) {
        // Ensure we're testing against the right Plutus version.
        // If not, we should fail early.
        assert_eq!(test_vector.meta.plutus_version, 3);

        let transaction: Transaction = cbor::decode(&test_vector.input.transaction_bytes).unwrap();

        let utxos = test_vector.input.utxo.clone().into();
        let tx_info = TxInfo::new(
            &transaction.body,
            &transaction.witnesses,
            transaction.tx_id(),
            &utxos,
            &0.into(),
            // These should probably be encoded in the TestVector itself
            &PREPROD_ERA_HISTORY,
            &PREPROD_GLOBAL_PARAMETERS,
        )
        .unwrap();

        let produced_contexts = tx_info
            .redeemers
            .iter()
            .map(|(key, entry)| {
                let script_context = ScriptContext::new(&tx_info, key, entry);
                let plutus_data = to_cbor(
                    &<ScriptContext<'_> as ToPlutusData<3>>::to_plutus_data(&script_context)
                        .expect("failed to encode as PlutusData"),
                );

                hex::encode(plutus_data)
            })
            .collect::<Vec<_>>();

        let found_match = produced_contexts.iter().any(|context| context == &test_vector.expectations.script_context);

        assert!(
            found_match,
            "No redeemer produced the expected script context: {}\nProduced script contexts: {}",
            test_vector.expectations.script_context,
            produced_contexts.join("\n\n")
        );
    }

    #[test]
    fn governance_quorum_uses_constr_encoding() {
        let action = GovernanceAction::UpdateCommittee(
            None,
            vec![],
            KeyValuePairs::default(),
            RationalNumber { numerator: 2, denominator: 4 },
        );

        let PlutusData::Constr(constr) = action.to_plutus_data().expect("governance action should encode") else {
            panic!("governance action should encode as a constructor")
        };

        let quorum = constr.fields.last().expect("update committee should contain quorum");
        let PlutusData::Constr(quorum) = quorum else { panic!("governance quorum should encode as a constructor") };

        assert_eq!(quorum.tag, 121);
        assert_eq!(
            quorum.fields.deref(),
            &[
                <u64 as ToPlutusData<3>>::to_plutus_data(&1).unwrap(),
                <u64 as ToPlutusData<3>>::to_plutus_data(&2).unwrap(),
            ]
        );
    }

    #[test]
    fn protocol_parameter_ratios_keep_array_encoding() {
        let ratio = RationalNumber { numerator: 2, denominator: 4 };

        let PlutusData::Array(values) = protocol_parameter_ratio(&ratio).expect("ratio should encode") else {
            panic!("protocol parameter ratio should encode as an array")
        };

        assert_eq!(
            values.deref(),
            &[
                <u64 as ToPlutusData<3>>::to_plutus_data(&1).unwrap(),
                <u64 as ToPlutusData<3>>::to_plutus_data(&2).unwrap(),
            ]
        );
    }
}
