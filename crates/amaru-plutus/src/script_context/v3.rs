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

use std::ops::Deref;

use amaru_kernel::{
    Address, AssetName, Bytes, Constitution, DRep, DRepVotingThresholds, DatumOption, ExUnitPrices,
    ExUnits, GovAction, Mint, PoolVotingThresholds, Proposal, ProposalId, ProtocolParamUpdate,
    RationalNumber, ScriptRef, StakeCredential, Value, Vote, from_alonzo_value,
};
use num::Integer;

use crate::{
    Constr, DEFAULT_TAG, MaybeIndefArray, ToConstrTag, ToPlutusData, constr,
    script_context::{
        AddrKeyhash, Certificate, DatumHash, KeyValuePairs, Lovelace, OutputRef, PlutusData,
        PolicyId, Redeemer, StakeAddress, TimeRange, TransactionId, TransactionInput,
        TransactionOutput, Voter,
    },
};

// Reference: https://github.com/IntersectMBO/plutus/blob/master/plutus-ledger-api/src/PlutusLedgerApi/V3/Data/Contexts.hs#L572
pub struct TxInfo {
    inputs: Vec<OutputRef>,
    reference_inputs: Vec<OutputRef>,
    outputs: Vec<TransactionOutput>,
    fee: Lovelace,
    mint: Mint,
    certificates: Vec<Certificate>,
    withdrawals: KeyValuePairs<StakeAddress, Lovelace>,
    valid_range: TimeRange,
    signatories: Vec<AddrKeyhash>,
    redeemers: KeyValuePairs<ScriptPurpose, Redeemer>,
    data: KeyValuePairs<DatumHash, PlutusData>,
    id: TransactionId,
    votes: KeyValuePairs<Voter, KeyValuePairs<ProposalId, Vote>>,
    proposal_procedures: Vec<Proposal>,
    current_treasury_amount: Option<Lovelace>,
    treasury_donation: Option<Lovelace>,
}

#[derive(Clone)]
pub struct ScriptPurpose(ScriptInfo);

impl Deref for ScriptPurpose {
    type Target = ScriptInfo;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Clone)]
pub enum ScriptInfo {
    Minting(PolicyId),
    Spending(TransactionInput, Option<DatumOption>),
    Rewarding(StakeCredential),
    Certifying(usize, Certificate),
    Voting(Voter),
    Proposing(usize, Proposal),
}

pub struct ScriptContext {
    tx_info: TxInfo,
    redeemer: Redeemer,
    script_info: ScriptInfo,
}

impl ToPlutusData<3> for ScriptContext {
    fn to_plutus_data(&self) -> PlutusData {
        constr!(v: 3, 0, self.tx_info, self.redeemer, self.script_info)
    }
}

impl ToPlutusData<3> for TxInfo {
    fn to_plutus_data(&self) -> PlutusData {
        constr!(v: 3, 0, self.inputs, self.reference_inputs, self.outputs, self.fee, self.mint, self.certificates, self.withdrawals, self.valid_range, self.signatories, self.redeemers, self.data, self.id, self.votes, self.proposal_procedures, self.current_treasury_amount, self.treasury_donation)
    }
}

impl ToPlutusData<3> for ScriptPurpose {
    fn to_plutus_data(&self) -> PlutusData {
        match self.deref() {
            ScriptInfo::Minting(policy_id) => constr!(v: 3, 0, policy_id),
            ScriptInfo::Spending(out_ref, _) => constr!(v: 3, 1, out_ref),
            ScriptInfo::Rewarding(stake_credential) => constr!(v: 3, 2, stake_credential),
            ScriptInfo::Certifying(ix, certificate) => constr!(v: 3, 3, ix, certificate),
            ScriptInfo::Voting(voter) => constr!(v: 3, 4, voter),
            ScriptInfo::Proposing(ix, procedure) => constr!(v: 3, 5, ix, procedure),
        }
    }
}

impl ToPlutusData<3> for ScriptInfo {
    fn to_plutus_data(&self) -> PlutusData {
        match self {
            ScriptInfo::Minting(policy_id) => constr!(v: 3, 0, policy_id),
            ScriptInfo::Spending(out_ref, datum) => {
                constr!(v: 3, 1, out_ref, datum)
            }
            ScriptInfo::Rewarding(stake_credential) => {
                constr!(v: 3, 2, stake_credential)
            }
            ScriptInfo::Certifying(ix, dcert) => constr!(v: 3, 3, ix, dcert),
            ScriptInfo::Voting(voter) => constr!(v: 3, 4, voter),
            ScriptInfo::Proposing(ix, procedure) => {
                constr!(v: 3, 5, ix, procedure)
            }
        }
    }
}

impl ToPlutusData<3> for OutputRef {
    fn to_plutus_data(&self) -> PlutusData {
        constr!(v: 3, 0, self.input, self.output)
    }
}

impl ToPlutusData<3> for TransactionInput {
    fn to_plutus_data(&self) -> PlutusData {
        constr!(v: 3, 0, self.transaction_id, self.index)
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
impl ToPlutusData<3> for TransactionOutput {
    fn to_plutus_data(&self) -> PlutusData {
        match self {
            amaru_kernel::PseudoTransactionOutput::Legacy(output) => {
                constr!(v: 3, 0, Address::from_bytes(&output.address).unwrap(), from_alonzo_value(output.amount.clone()).expect("illegal alonzo value"), output.datum_hash.map(DatumOption::Hash), None::<ScriptRef>)
            }
            amaru_kernel::PseudoTransactionOutput::PostAlonzo(output) => {
                constr!(v: 3, 0, Address::from_bytes(&output.address).unwrap(), output.value, output.datum_option, output.script_ref.as_ref().map(|s| s.clone().unwrap()))
            }
        }
    }
}

impl ToPlutusData<3> for Value {
    fn to_plutus_data(&self) -> PlutusData {
        fn ada_entry(coin: &u64) -> (PlutusData, PlutusData) {
            (
                <Bytes as ToPlutusData<3>>::to_plutus_data(&Bytes::from(vec![])),
                PlutusData::Map(KeyValuePairs::Def(vec![(
                    <AssetName as ToPlutusData<3>>::to_plutus_data(&AssetName::from(vec![])),
                    <u64 as ToPlutusData<3>>::to_plutus_data(coin),
                )])),
            )
        }

        let entries = match self {
            Value::Coin(coin) if *coin > 0 => vec![ada_entry(coin)],
            Value::Coin(_) => vec![],
            Value::Multiasset(coin, multiasset) => {
                let ada = (*coin > 0).then(|| ada_entry(coin));
                let multiasset_entries = multiasset.iter().map(|(policy_id, assets)| {
                    (
                        <PolicyId as ToPlutusData<3>>::to_plutus_data(policy_id),
                        PlutusData::Map(KeyValuePairs::Def(
                            assets
                                .iter()
                                .map(|(asset, amount)| {
                                    (
                                        <Bytes as ToPlutusData<3>>::to_plutus_data(asset),
                                        <u64 as ToPlutusData<3>>::to_plutus_data(&amount.into()),
                                    )
                                })
                                .collect(),
                        )),
                    )
                });
                ada.into_iter().chain(multiasset_entries).collect()
            }
        };

        PlutusData::Map(KeyValuePairs::Def(entries))
    }
}

impl ToPlutusData<3> for DRep {
    fn to_plutus_data(&self) -> PlutusData {
        match self {
            DRep::Key(hash) => constr!(v:3,0,StakeCredential::AddrKeyhash(*hash)),
            DRep::Script(hash) => constr!(v:3,0,StakeCredential::ScriptHash(*hash)),
            DRep::Abstain => constr!(1),
            DRep::NoConfidence => constr!(2),
        }
    }
}

impl ToPlutusData<3> for Certificate {
    fn to_plutus_data(&self) -> PlutusData {
        match self {
            Certificate::StakeRegistration(stake_credential) => {
                constr!(v: 3, 0, stake_credential, None::<PlutusData>)
            }
            Certificate::Reg(stake_credential, _) => {
                constr!(v: 3, 0, stake_credential, None::<PlutusData>)
            }
            Certificate::StakeDeregistration(stake_credential) => {
                constr!(v: 3, 1, stake_credential, None::<PlutusData>)
            }
            Certificate::UnReg(stake_credential, _) => {
                constr!(v: 3, 1, stake_credential, None::<PlutusData>)
            }
            Certificate::StakeDelegation(stake_credential, pool_id) => {
                constr!(v: 3, 2, stake_credential, constr!(v:3, 0, pool_id))
            }
            Certificate::VoteDeleg(stake_credential, drep) => {
                constr!(v: 3, 2, stake_credential, constr!(v:3, 1, drep))
            }
            Certificate::StakeVoteDeleg(stake_credential, pool_id, drep) => {
                constr!(v: 3, 2, stake_credential, constr!(v: 3, 2, pool_id, drep))
            }
            Certificate::StakeRegDeleg(stake_credential, pool_id, deposit) => {
                constr!(v: 3, 3, stake_credential, constr!(v:3, 0, pool_id), deposit)
            }
            Certificate::VoteRegDeleg(stake_credential, drep, deposit) => {
                constr!(v: 3, 3, stake_credential, constr!(v:3, 1, drep), deposit)
            }
            Certificate::StakeVoteRegDeleg(stake_credential, pool_id, drep, deposit) => {
                constr!(v: 3, 3, stake_credential, constr!(v: 3, 2, pool_id, drep), deposit)
            }
            Certificate::RegDRepCert(drep_credential, deposit, _anchor) => {
                constr!(v: 3, 4, drep_credential, deposit)
            }
            Certificate::UpdateDRepCert(drep_credential, _anchor) => {
                constr!(v: 3, 5, drep_credential)
            }
            Certificate::UnRegDRepCert(drep_credential, deposit) => {
                constr!(v: 3, 6, drep_credential, deposit)
            }
            Certificate::PoolRegistration {
                operator,
                vrf_keyhash,
                pledge: _,
                cost: _,
                margin: _,
                reward_account: _,
                pool_owners: _,
                relays: _,
                pool_metadata: _,
            } => {
                constr!(v: 3, 7, operator, vrf_keyhash)
            }
            Certificate::PoolRetirement(pool_keyhash, epoch) => {
                constr!(v: 3, 8, pool_keyhash, epoch)
            }
            Certificate::AuthCommitteeHot(cold_credential, hot_credential) => {
                constr!(v: 3, 9, cold_credential, hot_credential)
            }
            Certificate::ResignCommitteeCold(cold_credential, _anchor) => {
                constr!(v: 3, 10, cold_credential)
            }
        }
    }
}

impl ToPlutusData<3> for Voter {
    fn to_plutus_data(&self) -> PlutusData {
        match self {
            Voter::ConstitutionalCommitteeScript(hash) => {
                constr!(v: 3, 0, StakeCredential::ScriptHash(*hash))
            }
            Voter::ConstitutionalCommitteeKey(hash) => {
                constr!(v: 3, 0, StakeCredential::AddrKeyhash(*hash))
            }
            Voter::DRepScript(hash) => {
                constr!(v: 3, 1, StakeCredential::ScriptHash(*hash))
            }
            Voter::DRepKey(hash) => {
                constr!(v: 3, 1, StakeCredential::AddrKeyhash(*hash))
            }
            Voter::StakePoolKey(hash) => constr!(v: 3, 2, hash),
        }
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
impl ToPlutusData<3> for Proposal {
    fn to_plutus_data(&self) -> PlutusData {
        constr!(v: 3, self.deposit, Address::from_bytes(&self.reward_account).unwrap(), self.gov_action)
    }
}

#[allow(clippy::expect_used)]
impl ToPlutusData<3> for GovAction {
    fn to_plutus_data(&self) -> PlutusData {
        match self {
            GovAction::ParameterChange(previous_action, params, guardrail) => {
                constr!(v: 3, 0, previous_action, params.as_ref(), guardrail)
            }
            GovAction::HardForkInitiation(previous_action, version) => {
                constr!(v: 3,  1, previous_action, version)
            }
            GovAction::TreasuryWithdrawals(withdrawals, guardrail) => {
                constr!(v: 3, 2, KeyValuePairs::from(withdrawals.iter().map(|(reward_account, amount)| (Address::from_bytes(reward_account).expect("invalid stake addressin treasury withdrawal?"), *amount)).collect::<Vec<_>>()), guardrail)
            }
            GovAction::NoConfidence(previous_action) => {
                constr!(v: 3, 3, previous_action)
            }
            GovAction::UpdateCommittee(previous_action, removed, added, quorum) => {
                constr!(v: 3, 4, previous_action, removed.deref(), added, quorum)
            }
            GovAction::NewConstitution(previous_action, constitution) => {
                constr!(v: 3, 5, previous_action, constitution)
            }
            GovAction::Information => constr!(6),
        }
    }
}

impl ToPlutusData<3> for Constitution {
    fn to_plutus_data(&self) -> PlutusData {
        constr!(v: 3, 0, self.guardrail_script)
    }
}

impl ToPlutusData<3> for ProposalId {
    fn to_plutus_data(&self) -> PlutusData {
        constr!(v: 3, 0, self.transaction_id, self.action_index)
    }
}

impl ToPlutusData<3> for ProtocolParamUpdate {
    fn to_plutus_data(&self) -> PlutusData {
        let mut pparams = Vec::with_capacity(30);

        let mut push = |ix: usize, p: PlutusData| {
            pparams.push((<usize as ToPlutusData<3>>::to_plutus_data(&ix), p));
        };

        if let Some(p) = self.minfee_a {
            push(0, <u64 as ToPlutusData<3>>::to_plutus_data(&p));
        }

        if let Some(p) = self.minfee_b {
            push(1, <u64 as ToPlutusData<3>>::to_plutus_data(&p));
        }

        if let Some(p) = self.max_block_body_size {
            push(2, <u64 as ToPlutusData<3>>::to_plutus_data(&p));
        }

        if let Some(p) = self.max_transaction_size {
            push(3, <u64 as ToPlutusData<3>>::to_plutus_data(&p));
        }

        if let Some(p) = self.max_block_header_size {
            push(4, <u64 as ToPlutusData<3>>::to_plutus_data(&p));
        }

        if let Some(p) = self.key_deposit {
            push(5, <u64 as ToPlutusData<3>>::to_plutus_data(&p));
        }

        if let Some(p) = self.pool_deposit {
            push(6, <u64 as ToPlutusData<3>>::to_plutus_data(&p));
        }

        if let Some(p) = self.maximum_epoch {
            push(7, <u64 as ToPlutusData<3>>::to_plutus_data(&p));
        }

        if let Some(p) = self.desired_number_of_stake_pools {
            push(8, <u64 as ToPlutusData<3>>::to_plutus_data(&p));
        }

        if let Some(ref p) = self.pool_pledge_influence {
            push(9, p.to_plutus_data());
        }

        if let Some(ref p) = self.expansion_rate {
            push(10, p.to_plutus_data());
        }

        if let Some(ref p) = self.treasury_growth_rate {
            push(11, p.to_plutus_data());
        }

        if let Some(p) = self.min_pool_cost {
            push(16, <u64 as ToPlutusData<3>>::to_plutus_data(&p));
        }

        if let Some(p) = self.ada_per_utxo_byte {
            push(17, <u64 as ToPlutusData<3>>::to_plutus_data(&p));
        }

        // TODO: this is from Aiken, need to implement this
        #[allow(clippy::redundant_pattern_matching)]
        if let Some(_) = self.cost_models_for_script_languages {
            unimplemented!("TODO: ToPlutusData for cost models.");
        }

        if let Some(ref p) = self.execution_costs {
            push(19, p.to_plutus_data());
        }

        if let Some(p) = self.max_tx_ex_units {
            push(20, p.to_plutus_data());
        }

        if let Some(p) = self.max_block_ex_units {
            push(21, p.to_plutus_data());
        }

        if let Some(p) = self.max_value_size {
            push(22, <u64 as ToPlutusData<3>>::to_plutus_data(&p));
        }

        if let Some(p) = self.collateral_percentage {
            push(23, <u64 as ToPlutusData<3>>::to_plutus_data(&p));
        }

        if let Some(p) = self.max_collateral_inputs {
            push(24, <u64 as ToPlutusData<3>>::to_plutus_data(&p));
        }

        if let Some(ref p) = self.pool_voting_thresholds {
            push(25, p.to_plutus_data());
        }

        if let Some(ref p) = self.drep_voting_thresholds {
            push(26, p.to_plutus_data());
        }

        if let Some(p) = self.min_committee_size {
            push(27, <u64 as ToPlutusData<3>>::to_plutus_data(&p));
        }

        if let Some(p) = self.committee_term_limit {
            push(28, <u64 as ToPlutusData<3>>::to_plutus_data(&p));
        }

        if let Some(p) = self.governance_action_validity_period {
            push(29, <u64 as ToPlutusData<3>>::to_plutus_data(&p));
        }

        if let Some(p) = self.governance_action_deposit {
            push(30, <u64 as ToPlutusData<3>>::to_plutus_data(&p));
        }

        if let Some(p) = self.drep_deposit {
            push(31, <u64 as ToPlutusData<3>>::to_plutus_data(&p));
        }

        if let Some(p) = self.drep_inactivity_period {
            push(32, <u64 as ToPlutusData<3>>::to_plutus_data(&p));
        }

        if let Some(ref p) = self.minfee_refscript_cost_per_byte {
            push(33, p.to_plutus_data());
        }

        PlutusData::Map(KeyValuePairs::Def(pparams))
    }
}
// Do I need the tag encoding?
impl ToPlutusData<3> for RationalNumber {
    fn to_plutus_data(&self) -> PlutusData {
        let gcd = self.numerator.gcd(&self.denominator);
        <Vec<_> as ToPlutusData<3>>::to_plutus_data(&vec![
            self.numerator / gcd,
            self.denominator / gcd,
        ])
    }
}

impl ToPlutusData<3> for ExUnitPrices {
    fn to_plutus_data(&self) -> PlutusData {
        vec![&self.mem_price, &self.step_price].to_plutus_data()
    }
}

impl ToPlutusData<3> for PoolVotingThresholds {
    fn to_plutus_data(&self) -> PlutusData {
        vec![
            &self.motion_no_confidence,
            &self.committee_normal,
            &self.committee_no_confidence,
            &self.hard_fork_initiation,
            &self.security_voting_threshold,
        ]
        .to_plutus_data()
    }
}

impl ToPlutusData<3> for DRepVotingThresholds {
    fn to_plutus_data(&self) -> PlutusData {
        vec![
            &self.motion_no_confidence,
            &self.committee_normal,
            &self.committee_no_confidence,
            &self.update_constitution,
            &self.hard_fork_initiation,
            &self.pp_network_group,
            &self.pp_economic_group,
            &self.pp_technical_group,
            &self.pp_governance_group,
            &self.treasury_withdrawal,
        ]
        .to_plutus_data()
    }
}

impl ToPlutusData<3> for ExUnits {
    fn to_plutus_data(&self) -> PlutusData {
        <Vec<_> as ToPlutusData<3>>::to_plutus_data(&vec![&self.mem, &self.steps])
    }
}

impl ToPlutusData<3> for Vote {
    fn to_plutus_data(&self) -> PlutusData {
        match self {
            Vote::No => constr!(0),
            Vote::Yes => constr!(1),
            Vote::Abstain => constr!(2),
        }
    }
}
