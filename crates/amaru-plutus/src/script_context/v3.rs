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

use std::{collections::BTreeMap, ops::Deref};

use amaru_kernel::{
    Address, AssetName, Bytes, Constitution, DRep, DRepVotingThresholds, EraHistory, ExUnitPrices,
    ExUnits, GovAction, Hash, MemoizedTransactionOutput, MintedTransactionBody, MintedWitnessSet,
    PolicyId, PoolVotingThresholds, Proposal, ProposalId, ProposalIdAdapter, ProtocolParamUpdate,
    RationalNumber, ScriptPurpose as RedeemerTag, Slot, StakeCredential, TransactionInputAdapter,
    Vote, network::NetworkName, normalize_redeemers,
};
use amaru_slot_arithmetic::EraHistoryError;
use itertools::Itertools;
use num::Integer;
use thiserror::Error;

use crate::{
    ToPlutusData, constr, constr_v3,
    script_context::{
        Certificate, CurrencySymbol, Datums, KeyValuePairs, Lovelace, Mint, OutputRef, PlutusData,
        Redeemer, Redeemers, RequiredSigners, TimeRange, TransactionId, TransactionInput,
        TransactionOutput, Value, Voter, Votes, Withdrawals,
    },
};

#[derive(Debug, Error)]
pub enum PlutusV3Error {
    #[error("failed to translate input: {0}")]
    InputTranslationError(#[from] V3InputTranslationError),
    #[error("{0}")]
    UnspecifiedError(String),
    #[error("invalid validity range: {0}")]
    InvalidValidityRange(#[from] EraHistoryError),
    #[error("invalid redeemer at index {0}")]
    InvalidRedeemer(usize),
}

#[derive(Debug, Error)]
pub enum V3InputTranslationError {
    #[error("unknown input: {0}")]
    UnknownInput(TransactionInputAdapter),
    // TODO: verify this is actually needed, I'm not sure it is
    #[error("byron address not allowed: {0}")]
    ByronAddressNotAllowed(TransactionInputAdapter),
}

// Reference: https://github.com/IntersectMBO/plutus/blob/master/plutus-ledger-api/src/PlutusLedgerApi/V3/Data/Contexts.hs#L572
pub struct TxInfo {
    inputs: Vec<OutputRef>,
    reference_inputs: Vec<OutputRef>,
    outputs: Vec<TransactionOutput>,
    fee: Lovelace,
    mint: Mint,
    certificates: Vec<Certificate>,
    withdrawals: Withdrawals,
    valid_range: TimeRange,
    signatories: RequiredSigners,
    redeemers: Redeemers<ScriptPurpose>,
    data: Datums,
    id: TransactionId,
    votes: Votes,
    proposal_procedures: Vec<Proposal>,
    current_treasury_amount: Option<Lovelace>,
    treasury_donation: Option<Lovelace>,
}

// Much of this implementation is the same as V1 and, in turn, V2.
// It almost certainly makes sense to have a single struct that represents the superset
// of the `TxInfo` structs. Then each one should have a parameterized implementation to reduce (prevent?) illegal states
impl TxInfo {
    #[allow(clippy::expect_used)]
    pub fn new(
        tx: &MintedTransactionBody<'_>,
        id: &Hash<32>,
        witness_set: &MintedWitnessSet<'_>,
        utxo: &BTreeMap<TransactionInput, MemoizedTransactionOutput>,
        era_history: &EraHistory,
        slot: &Slot,
        network: NetworkName,
    ) -> Result<Self, PlutusV3Error> {
        let inputs =
            translate_inputs(&tx.inputs, utxo).map_err(PlutusV3Error::InputTranslationError)?;
        let reference_inputs = tx
            .reference_inputs
            .as_ref()
            .map(|reference_inputs| {
                translate_inputs(reference_inputs, utxo)
                    .map_err(PlutusV3Error::InputTranslationError)
            })
            .transpose()?
            .unwrap_or_default();

        let outputs = tx
            .outputs
            .iter()
            .map(|output| {
                MemoizedTransactionOutput::try_from(output.clone()).map(TransactionOutput::from)
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                PlutusV3Error::UnspecifiedError(format!(
                    "failed to parse transaction output: {}",
                    e
                ))
            })?;

        let mint = tx.mint.clone().map(|mint| mint.into()).unwrap_or_default();

        let certificates = tx
            .certificates
            .clone()
            .map(|set| set.to_vec())
            .unwrap_or_default();

        let withdrawals = tx
            .withdrawals
            .clone()
            .map(Withdrawals::try_from)
            .transpose()
            .map_err(PlutusV3Error::UnspecifiedError)?
            .unwrap_or_default();

        let valid_range = TimeRange::new(
            tx.validity_interval_start.map(Slot::from),
            tx.ttl.map(Slot::from),
            slot,
            era_history,
            network,
        )
        .map_err(PlutusV3Error::InvalidValidityRange)?;

        let proposal_procedures = tx
            .proposal_procedures
            .clone()
            .map(|proposals| proposals.to_vec())
            .unwrap_or_default();
        let votes = tx
            .voting_procedures
            .clone()
            .map(Votes::from)
            .unwrap_or_default();
        let redeemers = Redeemers(
            witness_set
                .redeemer
                .clone()
                .map(|redeemers| {
                    normalize_redeemers(redeemers.unwrap())
                        .into_iter()
                        .enumerate()
                        .map(|(ix, redeemer)| {
                            let purpose = ScriptPurpose::builder(
                                &redeemer,
                                &inputs[..],
                                &mint,
                                &withdrawals,
                                &certificates,
                                &proposal_procedures,
                                &votes,
                            )
                            .ok_or(PlutusV3Error::InvalidRedeemer(ix))?;

                            Ok((purpose, redeemer))
                        })
                        .collect::<Result<Vec<(ScriptPurpose, Redeemer)>, PlutusV3Error>>()
                })
                .transpose()?
                .unwrap_or_default(),
        );

        let datums = witness_set
            .plutus_data
            .clone()
            .map(Datums::from)
            .unwrap_or_default();

        Ok(Self {
            inputs,
            reference_inputs,
            outputs,
            fee: tx.fee,
            mint,
            certificates,
            withdrawals,
            valid_range,
            signatories: tx
                .required_signers
                .clone()
                .map(RequiredSigners::from)
                .unwrap_or_default(),
            redeemers,
            data: datums,
            id: *id,
            votes,
            proposal_procedures,
            current_treasury_amount: tx.treasury_value,
            treasury_donation: tx.donation.map(|donation| donation.into()),
        })
    }
}

fn translate_inputs(
    inputs: &[TransactionInput],
    utxo: &BTreeMap<TransactionInput, MemoizedTransactionOutput>,
) -> Result<Vec<OutputRef>, V3InputTranslationError> {
    inputs
        .iter()
        .sorted()
        .map(|input| {
            let utxo = match utxo.get(input) {
                Some(resolved) => resolved,
                None => {
                    return Err(V3InputTranslationError::UnknownInput(input.clone().into()));
                }
            };

            match utxo.address {
                Address::Byron(_) => {
                    return Err(V3InputTranslationError::ByronAddressNotAllowed(
                        input.clone().into(),
                    ));
                }
                Address::Stake(_) => {
                    unreachable!("stake address in UTxO")
                }
                _ => {}
            };

            Ok(OutputRef {
                input: input.clone(),
                output: utxo.clone().into(),
            })
        })
        .collect::<Result<Vec<_>, _>>()
}

pub type ScriptPurpose = ScriptInfo<()>;

impl ScriptPurpose {
    pub fn builder(
        redeemer: &Redeemer,
        inputs: &[OutputRef],
        mint: &Mint,
        withdrawals: &Withdrawals,
        certs: &[Certificate],
        proposal_procedures: &[Proposal],
        votes: &Votes,
    ) -> Option<Self> {
        let index = redeemer.index as usize;
        match redeemer.tag {
            RedeemerTag::Spend => inputs
                .get(index)
                .map(|output_ref| ScriptPurpose::Spending(output_ref.input.clone(), ())),
            RedeemerTag::Mint => mint
                .0
                .keys()
                .nth(index)
                .copied()
                .map(ScriptPurpose::Minting),
            RedeemerTag::Reward => withdrawals.0.keys().nth(index).map(|stake| {
                ScriptPurpose::Rewarding(match stake.0.payload() {
                    amaru_kernel::StakePayload::Stake(hash) => StakeCredential::AddrKeyhash(*hash),
                    amaru_kernel::StakePayload::Script(hash) => StakeCredential::ScriptHash(*hash),
                })
            }),
            RedeemerTag::Cert => certs
                .get(index)
                .map(|cert| ScriptPurpose::Certifying(index, cert.clone())),
            RedeemerTag::Vote => votes
                .0
                .keys()
                .nth(index)
                .cloned()
                .map(ScriptPurpose::Voting),
            RedeemerTag::Propose => proposal_procedures
                .get(index)
                .map(|p| ScriptPurpose::Proposing(index, p.clone())),
        }
    }

    pub fn to_script_info(self, data: Option<PlutusData>) -> ScriptInfo<Option<PlutusData>> {
        match self {
            ScriptInfo::Spending(input, _) => ScriptInfo::Spending(input, data),
            ScriptInfo::Minting(p) => ScriptInfo::Minting(p),
            ScriptInfo::Rewarding(s) => ScriptInfo::Rewarding(s),
            ScriptInfo::Certifying(i, c) => ScriptInfo::Certifying(i, c),
            ScriptInfo::Voting(v) => ScriptInfo::Voting(v),
            ScriptInfo::Proposing(i, p) => ScriptInfo::Proposing(i, p),
        }
    }
}

#[derive(Clone)]
pub enum ScriptInfo<T: Clone> {
    Minting(PolicyId),
    Spending(TransactionInput, T),
    Rewarding(StakeCredential),
    Certifying(usize, Certificate),
    Voting(Voter),
    Proposing(usize, Proposal),
}

pub struct ScriptContext {
    tx_info: TxInfo,
    redeemer: Redeemer,
    script_info: ScriptInfo<Option<PlutusData>>,
}

impl ScriptContext {
    pub fn new(tx_info: TxInfo, redeemer: &Redeemer, data: Option<PlutusData>) -> Option<Self> {
        let purpose = tx_info
            .redeemers
            .0
            .iter()
            .find_map(|(purpose, tx_redeemer)| {
                if redeemer.tag == tx_redeemer.tag && redeemer.index == tx_redeemer.index {
                    Some(purpose.clone())
                } else {
                    None
                }
            });

        purpose.map(|purpose| ScriptContext {
            tx_info,
            redeemer: redeemer.clone(),
            script_info: purpose.to_script_info(data),
        })
    }
}

impl ToPlutusData<3> for ScriptContext {
    fn to_plutus_data(&self) -> PlutusData {
        constr_v3!(0, [self.tx_info, self.redeemer, self.script_info])
    }
}

impl ToPlutusData<3> for TxInfo {
    fn to_plutus_data(&self) -> PlutusData {
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

impl ToPlutusData<3> for ScriptPurpose {
    fn to_plutus_data(&self) -> PlutusData {
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

impl ToPlutusData<3> for ScriptInfo<Option<PlutusData>> {
    fn to_plutus_data(&self) -> PlutusData {
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

impl ToPlutusData<3> for OutputRef {
    fn to_plutus_data(&self) -> PlutusData {
        constr_v3!(0, [self.input, self.output])
    }
}

impl ToPlutusData<3> for TransactionInput {
    fn to_plutus_data(&self) -> PlutusData {
        constr_v3!(0, [self.transaction_id, self.index])
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
impl ToPlutusData<3> for TransactionOutput {
    fn to_plutus_data(&self) -> PlutusData {
        constr_v3!(0, [self.address, self.value, self.datum, self.script])
    }
}

impl ToPlutusData<3> for Value {
    fn to_plutus_data(&self) -> PlutusData {
        if self.ada().is_none() {
            <BTreeMap<_, _> as ToPlutusData<3>>::to_plutus_data(
                &self
                    .0
                    .iter()
                    .filter(|(currency, _)| !matches!(currency, CurrencySymbol::Ada))
                    .collect::<BTreeMap<_, _>>(),
            )
        } else {
            <BTreeMap<_, _> as ToPlutusData<3>>::to_plutus_data(&self.0)
        }
    }
}
impl ToPlutusData<3> for amaru_kernel::Value {
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
            amaru_kernel::Value::Coin(coin) if *coin > 0 => vec![ada_entry(coin)],
            amaru_kernel::Value::Coin(_) => vec![],
            amaru_kernel::Value::Multiasset(coin, multiasset) => {
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
            DRep::Key(hash) => constr_v3!(0, [StakeCredential::AddrKeyhash(*hash)]),
            DRep::Script(hash) => constr_v3!(0, [StakeCredential::ScriptHash(*hash)]),
            DRep::Abstain => constr!(1),
            DRep::NoConfidence => constr!(2),
        }
    }
}

impl ToPlutusData<3> for Certificate {
    fn to_plutus_data(&self) -> PlutusData {
        match self {
            Certificate::StakeRegistration(stake_credential) => {
                constr_v3!(0, [stake_credential, None::<PlutusData>])
            }
            Certificate::Reg(stake_credential, _) => {
                constr_v3!(0, [stake_credential, None::<PlutusData>])
            }
            Certificate::StakeDeregistration(stake_credential) => {
                constr_v3!(1, [stake_credential, None::<PlutusData>])
            }
            Certificate::UnReg(stake_credential, _) => {
                constr_v3!(1, [stake_credential, None::<PlutusData>])
            }
            Certificate::StakeDelegation(stake_credential, pool_id) => {
                constr_v3!(2, [stake_credential, constr_v3!(0, [pool_id])])
            }
            Certificate::VoteDeleg(stake_credential, drep) => {
                constr_v3!(2, [stake_credential, constr_v3!(1, [drep])])
            }
            Certificate::StakeVoteDeleg(stake_credential, pool_id, drep) => {
                constr_v3!(2, [stake_credential, constr_v3!(2, [pool_id, drep])])
            }
            Certificate::StakeRegDeleg(stake_credential, pool_id, deposit) => {
                constr_v3!(3, [stake_credential, constr_v3!(0, [pool_id]), deposit])
            }
            Certificate::VoteRegDeleg(stake_credential, drep, deposit) => {
                constr_v3!(3, [stake_credential, constr_v3!(1, [drep]), deposit])
            }
            Certificate::StakeVoteRegDeleg(stake_credential, pool_id, drep, deposit) => {
                constr_v3!(
                    3,
                    [stake_credential, constr_v3!(2, [pool_id, drep]), deposit]
                )
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
            } => constr_v3!(7, [operator, vrf_keyhash]),
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
    fn to_plutus_data(&self) -> PlutusData {
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
    fn to_plutus_data(&self) -> PlutusData {
        constr_v3!(
            0,
            [
                self.deposit,
                Address::from_bytes(&self.reward_account).unwrap(),
                self.gov_action
            ]
        )
    }
}

#[allow(clippy::expect_used)]
impl ToPlutusData<3> for GovAction {
    fn to_plutus_data(&self) -> PlutusData {
        match self {
            GovAction::ParameterChange(previous_action, params, guardrail) => {
                constr_v3!(0, [previous_action, params.as_ref(), guardrail])
            }
            GovAction::HardForkInitiation(previous_action, version) => {
                constr_v3!(1, [previous_action, version])
            }
            GovAction::TreasuryWithdrawals(withdrawals, guardrail) => {
                constr_v3!(
                    2,
                    [
                        KeyValuePairs::from(
                            withdrawals
                                .iter()
                                .map(|(reward_account, amount)| (
                                    Address::from_bytes(reward_account)
                                        .expect("invalid stake addressin treasury withdrawal?"),
                                    *amount
                                ))
                                .collect::<Vec<_>>()
                        ),
                        guardrail
                    ]
                )
            }
            GovAction::NoConfidence(previous_action) => {
                constr_v3!(3, [previous_action])
            }
            GovAction::UpdateCommittee(previous_action, removed, added, quorum) => {
                // Check this -- in Aiken it uses a *different* encoding for quorum
                constr_v3!(4, [previous_action, removed.deref(), added, quorum])
            }
            GovAction::NewConstitution(previous_action, constitution) => {
                constr_v3!(5, [previous_action, constitution])
            }
            GovAction::Information => constr!(6),
        }
    }
}

impl ToPlutusData<3> for Constitution {
    fn to_plutus_data(&self) -> PlutusData {
        constr_v3!(0, [self.guardrail_script])
    }
}

impl ToPlutusData<3> for ProposalId {
    fn to_plutus_data(&self) -> PlutusData {
        constr_v3!(0, [self.transaction_id, self.action_index])
    }
}

impl ToPlutusData<3> for ProposalIdAdapter {
    fn to_plutus_data(&self) -> PlutusData {
        self.deref().to_plutus_data()
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

impl ToPlutusData<3> for Mint {
    fn to_plutus_data(&self) -> PlutusData {
        // Unlike in V2 and V3, we do not include zero ADA value
        <BTreeMap<_, _> as ToPlutusData<3>>::to_plutus_data(&self.0)
    }
}

impl ToPlutusData<3> for Withdrawals {
    fn to_plutus_data(&self) -> PlutusData {
        <BTreeMap<_, _> as ToPlutusData<3>>::to_plutus_data(&self.0)
    }
}

impl ToPlutusData<3> for Redeemers<ScriptPurpose> {
    fn to_plutus_data(&self) -> PlutusData {
        PlutusData::Map(KeyValuePairs::Def(
            self.0
                .iter()
                .map(|(purpose, data)| {
                    (
                        purpose.to_plutus_data(),
                        <Redeemer as ToPlutusData<3>>::to_plutus_data(data),
                    )
                })
                .collect(),
        ))
    }
}

impl ToPlutusData<3> for RequiredSigners {
    fn to_plutus_data(&self) -> PlutusData {
        let vec = self.0.iter().collect::<Vec<_>>();

        <Vec<_> as ToPlutusData<3>>::to_plutus_data(&vec)
    }
}

impl ToPlutusData<3> for Datums {
    fn to_plutus_data(&self) -> PlutusData {
        <BTreeMap<_, _> as ToPlutusData<3>>::to_plutus_data(&self.0)
    }
}

impl ToPlutusData<3> for Votes {
    fn to_plutus_data(&self) -> PlutusData {
        <BTreeMap<_, _> as ToPlutusData<3>>::to_plutus_data(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use amaru_kernel::{
        Fragment, KeepRaw, MemoizedTransactionOutput, MintedTransactionBody, MintedWitnessSet,
        OriginalHash, TransactionInput,
    };

    // These tests are ripped directly from Aiken.
    // They can, and should, be improved to follow a nicer testing pattern
    // like we use elsewhere in Amaru involving macros and `test_case`.

    fn fixture_tx_info(
        transaction: &str,
        witness_set: &str,
        inputs: &str,
        outputs: &str,
        slot: u64,
    ) -> TxInfo {
        let transaction_bytes = hex::decode(transaction).unwrap();
        let witness_set_bytes = hex::decode(witness_set).unwrap();
        let inputs_bytes = hex::decode(inputs).unwrap();
        let outputs_bytes = hex::decode(outputs).unwrap();

        let inputs = Vec::<TransactionInput>::decode_fragment(inputs_bytes.as_slice()).unwrap();
        let outputs =
            Vec::<MemoizedTransactionOutput>::decode_fragment(outputs_bytes.as_slice()).unwrap();
        let utxo: BTreeMap<TransactionInput, MemoizedTransactionOutput> = inputs
            .iter()
            .zip(outputs.iter())
            .map(|(input, output)| (input.clone(), output.clone()))
            .collect();

        let transaction: KeepRaw<'_, MintedTransactionBody<'_>> =
            minicbor::decode(&transaction_bytes).unwrap();
        let witness_set: MintedWitnessSet<'_> = minicbor::decode(&witness_set_bytes).unwrap();

        let tx_id = transaction.original_hash();

        TxInfo::new(
            &transaction.unwrap(),
            &tx_id,
            &witness_set,
            &utxo,
            NetworkName::Mainnet.into(),
            &slot.into(),
            NetworkName::Mainnet,
        )
        .unwrap()
    }

    fn empty_constr() -> PlutusData {
        PlutusData::Constr(Constr {
            tag: 121,
            any_constructor: Some(0),
            fields: MaybeIndefArray::Indef(vec![]),
        })
    }

    #[test]
    fn script_context_simple_send() {
        let datum = Some(empty_constr());

        let redeemer = Redeemer {
            tag: RedeemerTag::Spend,
            index: 0,
            data: empty_constr(),
            ex_units: ExUnits {
                mem: 1000000,
                steps: 100000000,
            },
        };

        let tx_info = fixture_tx_info(
            "A70081825820000000000000000000000000000000000000000000000000000000000000000000018182581D60111111111111111111111111111111111111111111111111111111111A3B9ACA0002182A0B5820FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF0D818258200000000000000000000000000000000000000000000000000000000000000000001082581D60000000000000000000000000000000000000000000000000000000001A3B9ACA001101",
            "A20581840000D87980821A000F42401A05F5E100078152510101003222253330044A229309B2B2B9A1",
            "81825820000000000000000000000000000000000000000000000000000000000000000000",
            "81a300581d7039f47fd3b388ef53c48f08de24766d3e55dade6cae908cc24e0f4f3e011a3b9aca00028201d81843d87980",
            0,
        );

        let script_context = ScriptContext::new(tx_info, &redeemer, datum).unwrap();

        // the snapshot that is being compared here is actually taken from the Aiken test (https://github.com/aiken-lang/aiken/blob/a8c032935dbaf4a1140e9d8be5c270acd32c9e8c/crates/uplc/src/tx/snapshots/uplc__tx__script_context__tests__script_context_simple_send.snap)
        insta::assert_debug_snapshot!(script_context.to_plutus_data())
    }
}
