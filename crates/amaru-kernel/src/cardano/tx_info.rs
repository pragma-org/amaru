// Copyright 2026 PRAGMA
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

use itertools::Itertools;
use thiserror::Error;

use crate::{
    BorrowedScript, Certificate, EraHistory, EraHistoryError, GlobalParameters, HasScriptHash, Hash, Lovelace,
    MemoizedTransactionOutput, OutputReference, PlutusDatums, PlutusMint, PlutusRedeemers, PlutusVotes,
    PlutusWithdrawals, Proposal, RedeemerEntry, RedeemerKey, RequiredSigners, ScriptContext, ScriptPurpose, TimeRange,
    TransactionBody, TransactionId, TransactionInput, Utxos, WithdrawalError, WitnessSet, size::SCRIPT,
};

/// An opaque type that represents the `TxInfo` field used in a [`ScriptContext`].
///
/// `TxInfo` is an in-memory representation of a Cardano transaction used in Plutus scripts.
///
/// Notably, it is not an exact mapping of the transaction on the ledger.
/// For example, bootstrap addresses are skipped in the inputs, reference inputs, and outputs.
#[derive(Debug)]
pub struct TxInfo<'a> {
    pub inputs: Vec<OutputReference<'a>>,
    pub reference_inputs: Vec<OutputReference<'a>>,
    pub outputs: Vec<&'a MemoizedTransactionOutput>,
    pub fee: Lovelace,
    pub mint: PlutusMint<'a>,
    pub certificates: Vec<&'a Certificate>,
    pub withdrawals: PlutusWithdrawals,
    pub valid_range: TimeRange,
    pub signatories: RequiredSigners,
    pub redeemers: PlutusRedeemers<'a>,
    pub data: PlutusDatums<'a>,
    pub id: TransactionId,
    pub votes: PlutusVotes<'a>,
    pub proposal_procedures: Vec<&'a Proposal>,
    pub current_treasury_amount: Option<Lovelace>,
    pub treasury_donation: Option<Lovelace>,
}

#[derive(Debug, Error)]
/// Represents possible errors that can occur during [`TxInfo` construction](TxInfo::new).
///
/// An occurance of this error should suggest a user error of one of two types:
/// - A poorly constructed transaction that should fail phase-one validation
/// - Incorrect chain state such as an incomplete UTxO slice, wrong network, or wrong slot value
pub enum TxInfoTranslationError {
    /// Some input was not in the provided [`Utxos`]
    #[error("missing input: {0}")]
    MissingInput(TransactionInput),
    /// Some withdrawal is poorly constructed
    #[error("invalid withdrawal: {0}")]
    InvalidWithdrawal(#[from] WithdrawalError),
    /// The validity interval cannot be converted to posix time
    #[error("invalid validity interval: {0}")]
    InvalidValidityInterval(#[from] EraHistoryError),
    /// Some redeemer is poorly constructed
    #[error("invalid redeemer at index {0}")]
    InvalidRedeemer(usize),
}

impl<'a> TxInfo<'a> {
    /// Construct a new `TxInfo` from a transaction and some additional context.
    ///
    /// This is a fallible operation which fails when some state can't be represented by `TxInfo`
    /// See [TxInfoTranslationError] for more.
    ///
    /// It's important to note that a successful construction of a `TxInfo` does not mean it can be serialized for all Plutus versions.
    /// For example, in Plutus V1, inputs that are locked by a bootstrap address are ignored, where as in V2 and V3 they are forbidden, resulting in an error.
    ///
    ///
    /// Version-specific errors will arise during serialization.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tx: &'a TransactionBody,
        witness_set: &'a WitnessSet,
        tx_id: TransactionId,
        utxos: &'a Utxos,
        slot: &crate::Slot,
        era_history: &EraHistory,
        global_parameters: &GlobalParameters,
    ) -> Result<Self, TxInfoTranslationError> {
        let mut scripts: BTreeMap<Hash<SCRIPT>, BorrowedScript<'_>> = BTreeMap::new();
        let inputs = Self::translate_inputs(&tx.inputs, utxos, &mut scripts)?;
        let reference_inputs = tx
            .reference_inputs
            .as_ref()
            .map(|ref_inputs| Self::translate_inputs(ref_inputs, utxos, &mut scripts))
            .transpose()?
            .unwrap_or_default();

        let outputs = tx.outputs.iter().collect::<Vec<_>>();

        let mint = tx.mint.as_ref().map(PlutusMint::from).unwrap_or_default();

        let certificates: Vec<&'a Certificate> =
            tx.certificates.as_ref().map(|set| set.iter().collect()).unwrap_or_default();

        let withdrawals = tx.withdrawals.as_ref().map(PlutusWithdrawals::try_from).transpose()?.unwrap_or_default();

        let valid_range =
            TimeRange::new(tx.validity_interval_start, tx.validity_interval_end, slot, era_history, global_parameters)?;

        let signatories = tx.required_signers.as_ref().map(RequiredSigners::from).unwrap_or_default();

        let proposal_procedures: Vec<_> =
            tx.proposals.as_ref().map(|proposals| proposals.iter().collect()).unwrap_or_default();

        let votes = tx.votes.as_ref().map(PlutusVotes::from).unwrap_or_default();

        if let Some(plutus_v1_scripts) = witness_set.plutus_v1_script.as_ref() {
            plutus_v1_scripts.iter().for_each(|script| {
                let script = BorrowedScript::PlutusV1(script);
                scripts.insert(script.script_hash(), script);
            });
        }

        if let Some(plutus_v2_scripts) = witness_set.plutus_v2_script.as_ref() {
            plutus_v2_scripts.iter().for_each(|script| {
                let script = BorrowedScript::PlutusV2(script);
                scripts.insert(script.script_hash(), script);
            });
        }

        if let Some(plutus_v3_scripts) = witness_set.plutus_v3_script.as_ref() {
            plutus_v3_scripts.iter().for_each(|script| {
                let script = BorrowedScript::PlutusV3(script);
                scripts.insert(script.script_hash(), script);
            });
        }

        let mut redeemers_map: BTreeMap<RedeemerKey, RedeemerEntry<'a>> = BTreeMap::new();
        if let Some(redeemers) = witness_set.redeemer.as_ref() {
            for (ix, (key, data, ex_units)) in PlutusRedeemers::iter_from(redeemers).enumerate() {
                let (purpose, script) = ScriptPurpose::builder(
                    &key,
                    &inputs[..],
                    &mint,
                    &withdrawals,
                    &certificates,
                    &proposal_procedures,
                    &votes,
                    &scripts,
                )
                .ok_or(TxInfoTranslationError::InvalidRedeemer(ix))?;

                // Plain insert is correct: RedeemerKey is primitive, so duplicate keys
                // collide on identity; the new value (carrying the new data/ex_units/script)
                // replaces the old. Matches Haskell `Map.fromList` last-wins semantics.
                redeemers_map.insert(key, RedeemerEntry { purpose, data, ex_units, script });
            }
        }

        let datums = witness_set.plutus_data.as_ref().map(PlutusDatums::from).unwrap_or_default();

        Ok(Self {
            inputs,
            reference_inputs,
            outputs,
            fee: tx.fee,
            mint,
            certificates,
            withdrawals,
            valid_range,
            signatories,
            redeemers: PlutusRedeemers::new(redeemers_map),
            data: datums,
            id: tx_id,
            votes,
            proposal_procedures,
            current_treasury_amount: tx.treasury_value,
            treasury_donation: tx.donation.map(|donation| donation.into()),
        })
    }

    /// Construct all script contexts for this TxInfo
    pub fn to_script_contexts(&self) -> Vec<(&RedeemerKey, ScriptContext<'_>, &BorrowedScript<'_>)> {
        self.redeemers.iter().map(|(key, entry)| (key, ScriptContext::new(self, key, entry), &entry.script)).collect()
    }

    fn translate_inputs(
        inputs: &'a [TransactionInput],
        utxos: &'a Utxos,
        scripts: &mut BTreeMap<Hash<SCRIPT>, BorrowedScript<'a>>,
    ) -> Result<Vec<OutputReference<'a>>, TxInfoTranslationError> {
        inputs
            .iter()
            .sorted()
            .map(|input| {
                let output_ref = utxos.resolve_input(input).ok_or(TxInfoTranslationError::MissingInput(*input))?;

                if let Some(script) = output_ref.output.script.as_ref() {
                    scripts.insert(script.script_hash(), BorrowedScript::from(script));
                };

                Ok(output_ref)
            })
            .collect::<Result<Vec<_>, _>>()
    }
}
