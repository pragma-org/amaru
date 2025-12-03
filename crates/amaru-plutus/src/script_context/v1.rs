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

use std::{borrow::Cow, collections::BTreeMap};

use amaru_kernel::{Address, AssetName, Hash, StakePayload, TransactionInput};

use crate::{
    IsKnownPlutusVersion, PlutusDataError, PlutusVersion, ToPlutusData, constr, constr_v1,
    script_context::{
        Certificate, CurrencySymbol, DatumOption, Datums, IsPrePlutusVersion3, Mint, OutputRef,
        PlutusData, ScriptContext, ScriptPurpose, StakeAddress, TransactionOutput, TxInfo, Value,
        Withdrawals,
    },
};

impl ToPlutusData<1> for OutputRef<'_> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        constr_v1!(0, [self.input, self.output])
    }
}

impl ToPlutusData<1> for ScriptContext<'_> {
    /// Convert a [`ScriptContext`] to the PlutusV1 representation of a `ScriptContext`.
    ///
    /// In PlutusV1 the `ScriptContext` contains:
    ///     - TxInfo
    ///     - ScriptPurpose
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        constr_v1!(0, [self.tx_info, self.script_purpose])
    }
}

impl ToPlutusData<1> for TxInfo<'_> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        let inputs = self
            .inputs
            .iter()
            .filter(|output_ref| !matches!(*output_ref.output.address, Address::Byron(..)))
            .collect::<Vec<_>>();

        let fee: Value<'_> = self.fee.into();

        constr_v1!(
            0,
            [
                inputs,
                self.outputs,
                fee,
                self.mint,
                self.certificates,
                self.withdrawals,
                self.valid_range,
                self.signatories,
                self.data,
                constr_v1!(0, [self.id])?,
            ]
        )
    }
}

impl<const V: u8> ToPlutusData<V> for ScriptPurpose<'_>
where
    PlutusVersion<V>: IsKnownPlutusVersion + IsPrePlutusVersion3,
{
    /// Serialize `ScriptPurpose` as PlutusData for PlutusV1 or PlutusV2.
    ///
    /// # Errors
    /// The following ScriptPurposes cannot be included in PlutusV1 or PlutusV2:
    /// - `ScriptPurpose::Voting`
    /// - `ScriptPurpose::Proposing`
    ///
    /// Serializing any of those will result in a `PlutusDataError`
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self {
            ScriptPurpose::Minting(policy_id) => constr_v1!(0, [policy_id]),
            ScriptPurpose::Spending(input, _) => constr_v1!(1, [input]),
            ScriptPurpose::Rewarding(stake_credential) => {
                constr_v1!(2, [constr_v1!(0, [stake_credential])?])
            }
            ScriptPurpose::Certifying(_ix, certificate) => constr_v1!(3, [certificate]),

            ScriptPurpose::Voting(_) => Err(PlutusDataError::unsupported_version(
                "voting purpose unsupported",
                V,
            )),
            ScriptPurpose::Proposing(_, _) => Err(PlutusDataError::unsupported_version(
                "proposing purpose unsupported",
                V,
            )),
        }
    }
}

impl<const V: u8> ToPlutusData<V> for Value<'_>
where
    PlutusVersion<V>: IsKnownPlutusVersion + IsPrePlutusVersion3,
{
    /// Serialize a `Value` as PlutusData for PlutusV1 and PlutusV2.
    ///
    /// Notably, in PlutusV1 and PlutusV2, `Value` must include a
    /// zero value lovelace asset, if there is no lovelace.
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        if self.0.contains_key(&CurrencySymbol::Lovelace) {
            self.0.to_plutus_data()
        } else {
            let mut map = self.0.clone();

            map.insert(
                CurrencySymbol::Lovelace,
                BTreeMap::from([(Cow::Owned(AssetName::from(vec![])), 0u64)]),
            );

            map.to_plutus_data()
        }
    }
}

impl ToPlutusData<1> for amaru_kernel::StakeAddress {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self.payload() {
            StakePayload::Stake(keyhash) => constr_v1!(0, [constr_v1!(0, [keyhash])?]),
            StakePayload::Script(script_hash) => constr_v1!(0, [constr_v1!(1, [script_hash])?]),
        }
    }
}

impl ToPlutusData<1> for StakeAddress {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        <amaru_kernel::StakeAddress as ToPlutusData<1>>::to_plutus_data(&self.0)
    }
}

impl<const V: u8> ToPlutusData<V> for TransactionInput
where
    PlutusVersion<V>: IsKnownPlutusVersion + IsPrePlutusVersion3,
{
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        constr!(0, [constr!(0, [self.transaction_id])?, self.index])
    }
}

#[allow(clippy::wildcard_enum_match_arm)]
impl<const V: u8> ToPlutusData<V> for Certificate
where
    PlutusVersion<V>: IsKnownPlutusVersion + IsPrePlutusVersion3,
{
    /// Serialize `Certificate` as PlutusData for PlutusV1 or PlutusV2.
    ///
    /// # Errors
    /// The following Certificates cannot be included in PlutusV1 or PlutusV2:
    /// - `Certificate::Reg`
    /// - `Certificate::UnReg`
    /// - `Certificate::VoteDeleg`
    /// - `Certificate::StakeVoteDeleg`
    /// - `Certificate::StakeRegDeleg`
    /// - `Certificate::VoteRegDeleg`
    /// - `Certificate::StakeVoteRegDeleg`
    /// - `Certificate::AuthCommitteeHot`
    /// - `Certificate::ResignCommitteeCold`
    /// - `Certificate::RegDRepCert`
    /// - `Certificate::UnRegDRepCert`
    /// - `Certificate::UpdateDRepCert`
    ///
    /// Serializing any of those will result in a `PlutusDataError`
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        match self {
            Certificate::StakeRegistration(stake_credential) => {
                constr!(0, [stake_credential])
            }
            Certificate::StakeDeregistration(stake_credential) => {
                constr!(1, [stake_credential])
            }
            Certificate::StakeDelegation(stake_credential, hash) => {
                constr!(2, [stake_credential, hash])
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
            } => constr!(3, [operator, vrf_keyhash]),
            Certificate::PoolRetirement(hash, epoch) => constr!(4, [hash, epoch]),
            certificate => Err(PlutusDataError::unsupported_version(
                format!("illegal certificate type: {certificate:?}"),
                V,
            )),
        }
    }
}

impl ToPlutusData<1> for TransactionOutput<'_> {
    #[allow(clippy::wildcard_enum_match_arm)]
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        constr_v1!(
            0,
            [
                self.address,
                self.value,
                match self.datum {
                    DatumOption::Hash(hash) => Some(*hash),
                    _ => None::<Hash<32>>,
                },
            ]
        )
    }
}

impl<const V: u8> ToPlutusData<V> for Mint<'_>
where
    PlutusVersion<V>: IsKnownPlutusVersion + IsPrePlutusVersion3,
{
    /// Serialize a `Mint` as PlutusData for PlutusV1 and PlutusV2.
    ///
    /// Notably, in PlutusV1 and PlutusV2, `Mint` must include a
    /// zero value lovelace asset
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        let mut mint = self
            .0
            .iter()
            .map(|(policy, multiasset)| (policy.to_vec(), multiasset))
            .collect::<BTreeMap<_, _>>();

        let ada_bundle = BTreeMap::from([(Cow::Owned(vec![].into()), 0)]);
        mint.insert(vec![], &ada_bundle);

        <BTreeMap<_, _> as ToPlutusData<1>>::to_plutus_data(&mint)
    }
}

impl ToPlutusData<1> for Withdrawals {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        <Vec<_> as ToPlutusData<1>>::to_plutus_data(
            &self
                .0
                .iter()
                .map(|(address, coin)| Ok((address, *coin)))
                .collect::<Result<Vec<_>, _>>()?,
        )
    }
}

impl ToPlutusData<1> for Datums<'_> {
    fn to_plutus_data(&self) -> Result<PlutusData, PlutusDataError> {
        <Vec<_> as ToPlutusData<1>>::to_plutus_data(&self.0.iter().collect::<Vec<_>>())
    }
}

// This test logic is basically 100% duplicated with v3. Should be able to simplify.
#[cfg(test)]
mod tests {
    use std::ops::Deref;

    use super::super::test_vectors::{self, TestVector};
    use super::*;
    use amaru_kernel::network::NetworkName;
    use amaru_kernel::{MintedTx, OriginalHash, normalize_redeemers, to_cbor};
    use test_case::test_case;

    const PLUTUS_VERSION: u8 = 1;

    macro_rules! fixture {
        ($title:literal) => {
            test_vectors::get_test_vector($title, PLUTUS_VERSION)
        };
    }

    #[test_case(fixture!("simple_send"); "simple send")]
    #[test_case(fixture!("mint"); "mint")]
    fn test_plutus_v1(test_vector: &TestVector) {
        // Ensure we're testing against the right Plutus version.
        // If not, we should fail early.
        assert_eq!(test_vector.meta.plutus_version, PLUTUS_VERSION);

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
                )
                .unwrap();

                let script_context = ScriptContext::new(&tx_info, redeemer).unwrap();
                let plutus_data = to_cbor(
                    &<ScriptContext<'_> as ToPlutusData<1>>::to_plutus_data(&script_context)
                        .expect("failed to ScriptContext convert to PlutusData"),
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
