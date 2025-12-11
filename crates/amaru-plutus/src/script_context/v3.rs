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

use std::{borrow::Cow, collections::BTreeMap, ops::Deref};

use amaru_kernel::{
    Address, AssetName, Bytes, Certificate as PallasCertificate, Constitution, DRep,
    DRepVotingThresholds, ExUnitPrices, ExUnits, GovAction, PolicyId, PoolVotingThresholds,
    Proposal, ProposalId, ProposalIdAdapter, ProtocolParamUpdate, RationalNumber, StakeCredential,
    StakePayload, Vote,
};
use num::Integer;

use crate::{
    PlutusDataError, ToPlutusData, constr, constr_v3,
    script_context::{
        Certificate, CurrencySymbol, Datums, KeyValuePairs, Mint, OutputRef, PlutusData, Redeemers,
        ScriptContext, ScriptInfo, ScriptPurpose, StakeAddress, TransactionInput,
        TransactionOutput, TxInfo, Value, Voter, Votes, Withdrawals,
    },
};

impl ToPlutusData<3> for OutputRef<'_> {
    /// Serialize an `OutputRef` as PlutusData for PlutusV3.
    ///
    /// # Errors
    /// If the UTxO is locked at a bootstrap address, this will return a `PlutusDataError`.
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        if let Address::Byron(_) = *self.output.address {
            return Err(PlutusDataError::unsupported_version(
                "byron address included in OutputRef",
                3,
            ));
        }

        constr_v3!(0, [self.input, self.output])
    }
}

impl ToPlutusData<3> for ScriptContext<'_> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        constr_v3!(
            0,
            [
                self.tx_info,
                self.redeemer,
                self.script_purpose.to_script_info(self.datum)
            ]
        )
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

impl ToPlutusData<3> for TransactionOutput<'_> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        constr_v3!(0, [self.address, self.value, self.datum, self.script])
    }
}

impl ToPlutusData<3> for Value<'_> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        if self.ada().is_none() {
            <BTreeMap<_, _> as ToPlutusData<3>>::to_plutus_data(
                &self
                    .0
                    .iter()
                    .filter(|(currency, _)| !matches!(currency, CurrencySymbol::Lovelace))
                    .collect::<BTreeMap<_, _>>(),
            )
        } else {
            <BTreeMap<_, _> as ToPlutusData<3>>::to_plutus_data(&self.0)
        }
    }
}
impl ToPlutusData<3> for amaru_kernel::Value {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        fn ada_entry(coin: &u64) -> Result<(PlutusData, PlutusData), PlutusDataError> {
            Ok((
                <Bytes as ToPlutusData<3>>::to_plutus_data(&Bytes::from(vec![]))?,
                PlutusData::Map(KeyValuePairs::Def(vec![(
                    <AssetName as ToPlutusData<3>>::to_plutus_data(&AssetName::from(vec![]))?,
                    <u64 as ToPlutusData<3>>::to_plutus_data(coin)?,
                )])),
            ))
        }

        let entries = match self {
            amaru_kernel::Value::Coin(coin) if *coin > 0 => Ok(vec![ada_entry(coin)?]),
            amaru_kernel::Value::Coin(_) => Ok(vec![]),
            amaru_kernel::Value::Multiasset(coin, multiasset) => {
                let ada = (*coin > 0).then(|| ada_entry(coin)).transpose()?;
                let multiasset_entries = multiasset
                    .iter()
                    .map(|(policy_id, assets)| {
                        Ok((
                            <PolicyId as ToPlutusData<3>>::to_plutus_data(policy_id)?,
                            PlutusData::Map(KeyValuePairs::Def(
                                assets
                                    .iter()
                                    .map(|(asset, amount)| {
                                        Ok((
                                            <Bytes as ToPlutusData<3>>::to_plutus_data(asset)?,
                                            <u64 as ToPlutusData<3>>::to_plutus_data(
                                                &amount.into(),
                                            )?,
                                        ))
                                    })
                                    .collect::<Result<Vec<_>, _>>()?,
                            )),
                        ))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(ada.into_iter().chain(multiasset_entries).collect())
            }
        }?;

        Ok(PlutusData::Map(KeyValuePairs::Def(entries)))
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

impl ToPlutusData<3> for Certificate<'_> {
    /// There is a bug in protocol version 9 that omitted the deposit valeus of new certificates.
    /// This was fixed in protocol version 10, but we must make sure that, for protocol version 9, the bug is included
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self.certificate {
            PallasCertificate::StakeRegistration(stake_credential) => {
                constr_v3!(0, [stake_credential, None::<PlutusData>])
            }
            PallasCertificate::Reg(stake_credential, coin) => {
                if self.protocol_verison.0 > 9 {
                    constr_v3!(0, [stake_credential, Some(coin)])
                } else {
                    constr_v3!(0, [stake_credential, None::<PlutusData>])
                }
            }
            PallasCertificate::StakeDeregistration(stake_credential) => {
                constr_v3!(1, [stake_credential, None::<PlutusData>])
            }
            PallasCertificate::UnReg(stake_credential, coin) => {
                if self.protocol_verison.0 > 9 {
                    constr_v3!(1, [stake_credential, Some(coin)])
                } else {
                    constr_v3!(1, [stake_credential, None::<PlutusData>])
                }
            }
            PallasCertificate::StakeDelegation(stake_credential, pool_id) => {
                constr_v3!(2, [stake_credential, constr_v3!(0, [pool_id])?])
            }
            PallasCertificate::VoteDeleg(stake_credential, drep) => {
                constr_v3!(2, [stake_credential, constr_v3!(1, [drep])?])
            }
            PallasCertificate::StakeVoteDeleg(stake_credential, pool_id, drep) => {
                constr_v3!(2, [stake_credential, constr_v3!(2, [pool_id, drep])?])
            }
            PallasCertificate::StakeRegDeleg(stake_credential, pool_id, deposit) => {
                constr_v3!(3, [stake_credential, constr_v3!(0, [pool_id])?, deposit])
            }
            PallasCertificate::VoteRegDeleg(stake_credential, drep, deposit) => {
                constr_v3!(3, [stake_credential, constr_v3!(1, [drep])?, deposit])
            }
            PallasCertificate::StakeVoteRegDeleg(stake_credential, pool_id, drep, deposit) => {
                constr_v3!(
                    3,
                    [stake_credential, constr_v3!(2, [pool_id, drep])?, deposit]
                )
            }
            PallasCertificate::RegDRepCert(drep_credential, deposit, _anchor) => {
                constr_v3!(4, [drep_credential, deposit])
            }
            PallasCertificate::UpdateDRepCert(drep_credential, _anchor) => {
                constr_v3!(5, [drep_credential])
            }
            PallasCertificate::UnRegDRepCert(drep_credential, deposit) => {
                constr_v3!(6, [drep_credential, deposit])
            }
            PallasCertificate::PoolRegistration {
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
            PallasCertificate::PoolRetirement(pool_keyhash, epoch) => {
                constr_v3!(8, [pool_keyhash, epoch])
            }
            PallasCertificate::AuthCommitteeHot(cold_credential, hot_credential) => {
                constr_v3!(9, [cold_credential, hot_credential])
            }
            PallasCertificate::ResignCommitteeCold(cold_credential, _anchor) => {
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
    /// Serializes a `GovAction` to PlutusData for PlutusV3.
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
            GovAction::ParameterChange(previous_action, params, guardrail) => {
                constr_v3!(0, [previous_action, params.as_ref(), guardrail])
            }
            GovAction::HardForkInitiation(previous_action, version) => {
                constr_v3!(1, [previous_action, version])
            }
            GovAction::TreasuryWithdrawals(withdrawals, guardrail) => {
                let withdrawals = withdrawals
                    .iter()
                    .map(|(reward_account, amount)| {
                        let reward_address = if let Ok(Address::Stake(reward_address)) =
                            Address::from_bytes(reward_account)
                        {
                            Ok(reward_address)
                        } else {
                            Err(PlutusDataError::Custom(
                                "invalid stake address in treasury withdrawal?".into(),
                            ))
                        }?;

                        Ok((reward_address, *amount))
                    })
                    .collect::<Result<Vec<(_, _)>, _>>()?;

                constr_v3!(2, [KeyValuePairs::from(withdrawals), guardrail])
            }
            GovAction::NoConfidence(previous_action) => {
                constr_v3!(3, [previous_action])
            }
            GovAction::UpdateCommittee(previous_action, removed, added, quorum) => {
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
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        constr_v3!(0, [self.guardrail_script])
    }
}

impl ToPlutusData<3> for ProposalId {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        constr_v3!(0, [self.transaction_id, self.action_index])
    }
}

impl ToPlutusData<3> for ProposalIdAdapter<'_> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        self.deref().to_plutus_data()
    }
}

impl ToPlutusData<3> for ProtocolParamUpdate {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        let mut pparams = Vec::with_capacity(30);

        let mut push =
            |ix: usize, p: Result<PlutusData, PlutusDataError>| -> Result<(), PlutusDataError> {
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
            push(9, p.to_plutus_data())?;
        }

        if let Some(ref p) = self.expansion_rate {
            push(10, p.to_plutus_data())?;
        }

        if let Some(ref p) = self.treasury_growth_rate {
            push(11, p.to_plutus_data())?;
        }

        if let Some(p) = self.min_pool_cost {
            push(16, <u64 as ToPlutusData<3>>::to_plutus_data(&p))?;
        }

        if let Some(p) = self.ada_per_utxo_byte {
            push(17, <u64 as ToPlutusData<3>>::to_plutus_data(&p))?;
        }

        // TODO: this is from Aiken, need to implement this
        #[allow(clippy::redundant_pattern_matching)]
        if let Some(_) = self.cost_models_for_script_languages {
            unimplemented!("TODO: ToPlutusData for cost models.");
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
            push(33, p.to_plutus_data())?;
        }

        Ok(PlutusData::Map(KeyValuePairs::Def(pparams)))
    }
}

impl ToPlutusData<3> for RationalNumber {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        let gcd = self.numerator.gcd(&self.denominator);
        <Vec<_> as ToPlutusData<3>>::to_plutus_data(&vec![
            self.numerator / gcd,
            self.denominator / gcd,
        ])
    }
}

impl ToPlutusData<3> for ExUnitPrices {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        vec![&self.mem_price, &self.step_price].to_plutus_data()
    }
}

impl ToPlutusData<3> for PoolVotingThresholds {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
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
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
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

impl ToPlutusData<3> for Mint<'_> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        <BTreeMap<_, _> as ToPlutusData<3>>::to_plutus_data(&self.0)
    }
}

impl ToPlutusData<3> for Withdrawals {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        <BTreeMap<_, _> as ToPlutusData<3>>::to_plutus_data(&self.0)
    }
}

impl ToPlutusData<3> for Redeemers<'_, ScriptPurpose<'_>> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        let converted: Result<Vec<_>, _> = self
            .0
            .iter()
            .map(|(purpose, data)| {
                Ok((
                    <ScriptPurpose<'_> as ToPlutusData<3>>::to_plutus_data(purpose)?,
                    <Cow<'_, _> as ToPlutusData<3>>::to_plutus_data(data)?,
                ))
            })
            .collect();

        Ok(PlutusData::Map(KeyValuePairs::Def(converted?)))
    }
}

impl ToPlutusData<3> for Datums<'_> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        <BTreeMap<_, _> as ToPlutusData<3>>::to_plutus_data(&self.0)
    }
}

impl ToPlutusData<3> for Votes<'_> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        self.0.to_plutus_data()
    }
}

impl ToPlutusData<3> for amaru_kernel::StakeAddress {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self.payload() {
            StakePayload::Stake(keyhash) => constr_v3!(0, [keyhash]),
            StakePayload::Script(script_hash) => constr_v3!(1, [script_hash]),
        }
    }
}

impl ToPlutusData<3> for StakeAddress {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        <amaru_kernel::StakeAddress as ToPlutusData<3>>::to_plutus_data(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_vectors::{self, TestVector};
    use super::*;
    use amaru_kernel::network::NetworkName;
    use amaru_kernel::{MintedTx, OriginalHash, PROTOCOL_VERSION_10, normalize_redeemers, to_cbor};
    use test_case::test_case;

    macro_rules! fixture {
        ($title:literal) => {
            test_vectors::get_test_vector($title, 3)
        };
    }

    #[test_case(fixture!("simple_send"); "simple send")]
    #[test_case(fixture!("simple_spend_no_datum"); "simple spend no datum")]
    #[test_case(fixture!("mint"); "mint")]
    #[test_case(fixture!("certificates_v10"); "certificates (protocol ver 10")]
    // The following test is commented out because we are disregarding protocol version 9.
    // See the comment on the `ToPlutusData` implementation for `Certificate` for more information
    // #[test_case(fixture!("certificates_v9"); "certificates (protocol ver 9")]
    fn test_plutus_v3(test_vector: &TestVector) {
        // Ensure we're testing against the right Plutus version.
        // If not, we should fail early.
        assert_eq!(test_vector.meta.plutus_version, 3);

        // this should probably be encoded in the TestVector itself
        let network = NetworkName::Preprod;

        let transaction: MintedTx<'_> =
            minicbor::decode(&test_vector.input.transaction_bytes).unwrap();

        let redeemers = normalize_redeemers(
            transaction
                .transaction_witness_set
                .redeemer
                .as_ref()
                .expect("no redeemers provided")
                .deref(),
        );

        let produced_contexts = redeemers
            .iter()
            .map(|redeemer| {
                let utxos = test_vector.input.utxo.clone().into();
                let tx_info = TxInfo::new(
                    &transaction.transaction_body,
                    &transaction.transaction_witness_set,
                    &transaction.transaction_body.original_hash(),
                    &utxos,
                    &0.into(),
                    network,
                    network.into(),
                    PROTOCOL_VERSION_10,
                )
                .unwrap();

                let script_context = ScriptContext::new(&tx_info, redeemer).unwrap();
                let plutus_data = to_cbor(
                    &<ScriptContext<'_> as ToPlutusData<3>>::to_plutus_data(&script_context)
                        .expect("failed to encode as PlutusData"),
                );

                hex::encode(plutus_data)
            })
            .collect::<Vec<_>>();

        let found_match = produced_contexts
            .iter()
            .any(|context| context == &test_vector.expectations.script_context);

        assert!(
            found_match,
            "No redeemer produced the expected script context: {}\nProduced script contexts: {}",
            test_vector.expectations.script_context,
            produced_contexts.join("\n\n")
        );
    }
}
