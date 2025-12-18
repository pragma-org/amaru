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

use amaru_kernel::{
    AddrKeyhash, Address, AddressError, AlonzoValue, AssetName, Bytes, CborWrap,
    Certificate as PallasCertificate, ComputeHash, DatumHash, EraHistory, ExUnits, HasIndex,
    HasOwnership, HasScriptHash, Hash, KeepRaw, KeyValuePairs, Lovelace, MemoizedDatum,
    MemoizedScript, MemoizedTransactionOutput, MintedDatumOption, MintedScriptRef,
    MintedTransactionBody, MintedTransactionOutput, MintedWitnessSet, NativeScript, Network,
    NonEmptyKeyValuePairs, NonEmptySet, Nullable, OrderedRedeemer, PlutusData, PlutusScript,
    PolicyId, Proposal, ProposalIdAdapter, ProtocolVersion, PseudoScript, Redeemer, RewardAccount,
    ScriptPurpose as RedeemerTag, Slot, StakeCredential, StakePayload, TransactionId,
    TransactionInput, TransactionInputAdapter, Vote, Voter, VotingProcedures, network::NetworkName,
    normalize_redeemers, protocol_parameters::GlobalParameters,
};
use amaru_slot_arithmetic::{EraHistoryError, TimeMs};
use itertools::Itertools;
use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    ops::Deref,
};
use thiserror::Error;

pub mod v1;
pub mod v2;
pub mod v3;

pub trait IsPrePlutusVersion3 {}
impl IsPrePlutusVersion3 for PlutusVersion<1> {}
impl IsPrePlutusVersion3 for PlutusVersion<2> {}

use crate::{IsKnownPlutusVersion, PlutusDataError, PlutusVersion, ToPlutusData};

/// One of the arguments passed to a Plutus validator.
///
///
/// It contains information about the transaction which is being validated, and the specific script which is being run.
///
/// A `ScriptContext` can only be constructed via the [`ScriptContext::new`](Self::new) function.
///
/// The serialized representation of `ScriptContext` may be different for each `PlutusVersion`,
/// so it is important to specify the correct `PlutusVersion` when serializing.
pub struct ScriptContext<'a> {
    tx_info: &'a TxInfo<'a>,
    // TODO: this should not be exposed, this is just for testing purposes atm
    pub redeemer: &'a Redeemer,
    datum: Option<&'a PlutusData>,
    script_purpose: &'a ScriptPurpose<'a>,
}

impl<'a> ScriptContext<'a> {
    /// Construct a new [`ScriptContext`] for a specific script execution (specified by the `Redeemer`).
    ///
    /// Returns `None` if the provided `Redeemer` does not exist in the `TxInfo`
    pub fn new(tx_info: &'a TxInfo<'a>, redeemer: &'a Redeemer) -> Option<Self> {
        let purpose = tx_info
            .redeemers
            .0
            .iter()
            .find_map(|(purpose, tx_redeemer)| {
                if redeemer.tag == tx_redeemer.tag && redeemer.index == tx_redeemer.index {
                    Some(purpose)
                } else {
                    None
                }
            });

        let datum = if amaru_kernel::ScriptPurpose::Spend == redeemer.tag {
            tx_info
                .inputs
                .get(redeemer.index as usize)
                .and_then(|output_ref| match output_ref.output.datum {
                    DatumOption::None => None,
                    DatumOption::Hash(hash) => tx_info.data.0.get(hash).copied(),
                    DatumOption::Inline(plutus_data) => Some(plutus_data),
                })
        } else {
            None
        };

        purpose.map(|script_purpose| ScriptContext {
            tx_info,
            redeemer,
            datum,
            script_purpose,
        })
    }

    /// Serialize `ScriptContext` to a list of arguments to be passed to a Plutus validator.
    ///
    /// For both PlutusV1 and PlutusV2, the list consists of:
    /// `[datum?, redeemer, script_context]`
    ///
    /// For PlutusV3 the lists consists of:
    /// `[script_context]`
    pub fn to_script_args<const V: u8>(
        &self,
        _version: PlutusVersion<V>,
    ) -> Result<Vec<PlutusData>, PlutusDataError>
    where
        PlutusVersion<V>: IsKnownPlutusVersion,
    {
        match V {
            1 => self.v1_script_args(),
            2 => self.v2_script_args(),
            3 => self.v3_script_args(),
            _ => unreachable!("unknown PlutusVersion passed to to_script_args"),
        }
    }

    pub fn budget(&self) -> &ExUnits {
        &self.redeemer.ex_units
    }

    fn v1_script_args(&self) -> Result<Vec<PlutusData>, PlutusDataError> {
        let mut args = vec![];
        if let Some(datum) = self.datum {
            args.push(datum.clone());
        }

        args.push(self.redeemer.data.clone());
        args.push(<Self as ToPlutusData<1>>::to_plutus_data(self)?);

        Ok(args)
    }

    fn v2_script_args(&self) -> Result<Vec<PlutusData>, PlutusDataError> {
        let mut args = vec![];
        if let Some(datum) = self.datum {
            args.push(datum.clone());
        }
        args.push(self.redeemer.data.clone());
        args.push(<Self as ToPlutusData<2>>::to_plutus_data(self)?);

        Ok(args)
    }

    fn v3_script_args(&self) -> Result<Vec<PlutusData>, PlutusDataError> {
        Ok(vec![<Self as ToPlutusData<3>>::to_plutus_data(self)?])
    }
}

/// An opaque type that represents the `TxInfo` field used in a [`ScriptContext`].
///
/// `TxInfo` is an in-memory representation of a Cardano transaction used in Plutus scripts.
///
/// Notably, it is not an exact mapping of the transaction on the ledger.
/// For example, bootstrap addresses are skipped in the inputs, reference inputs, and outputs.
pub struct TxInfo<'a> {
    inputs: Vec<OutputRef<'a>>,
    reference_inputs: Vec<OutputRef<'a>>,
    outputs: Vec<TransactionOutput<'a>>,
    fee: Lovelace,
    mint: Mint<'a>,
    certificates: Vec<Certificate<'a>>,
    withdrawals: Withdrawals,
    valid_range: TimeRange,
    signatories: RequiredSigners,
    redeemers: Redeemers<'a, ScriptPurpose<'a>>,
    data: Datums<'a>,
    id: TransactionId,
    votes: Votes<'a>,
    proposal_procedures: Vec<&'a Proposal>,
    current_treasury_amount: Option<Lovelace>,
    treasury_donation: Option<Lovelace>,
    script_table: BTreeMap<OrderedRedeemer<'a>, Script<'a>>,
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
    MissingInput(TransactionInputAdapter),
    /// Some output is poorly constructed
    #[error("invalid output: {0}")]
    InvalidOutput(#[from] TransactionOutputError),
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
        tx: &'a MintedTransactionBody<'_>,
        witness_set: &'a MintedWitnessSet<'_>,
        tx_id: &Hash<32>,
        utxos: &'a Utxos,
        slot: &Slot,
        network: NetworkName,
        era_history: &EraHistory,
        protocol_version: ProtocolVersion,
    ) -> Result<Self, TxInfoTranslationError> {
        let mut scripts: BTreeMap<Hash<28>, Script<'_>> = BTreeMap::new();
        let inputs = Self::translate_inputs(&tx.inputs, utxos, &mut scripts)?;
        let reference_inputs = tx
            .reference_inputs
            .as_ref()
            .map(|ref_inputs| Self::translate_inputs(ref_inputs, utxos, &mut scripts))
            .transpose()?
            .unwrap_or_default();

        let outputs = tx
            .outputs
            .iter()
            .map(TransactionOutput::try_from)
            .collect::<Result<Vec<_>, _>>()?;

        let mint = tx.mint.as_ref().map(Mint::from).unwrap_or_default();

        let certificates: Vec<Certificate<'a>> = tx
            .certificates
            .as_ref()
            .map(|set| {
                set.iter()
                    .map(|certificate| Certificate {
                        protocol_verison: protocol_version,
                        certificate,
                    })
                    .collect()
            })
            .unwrap_or_default();

        let withdrawals = tx
            .withdrawals
            .as_ref()
            .map(Withdrawals::try_from)
            .transpose()?
            .unwrap_or_default();

        let valid_range = TimeRange::new(
            tx.validity_interval_start.map(Slot::from),
            tx.ttl.map(Slot::from),
            slot,
            era_history,
            network,
        )?;

        let signatories = tx
            .required_signers
            .as_ref()
            .map(RequiredSigners::from)
            .unwrap_or_default();

        let proposal_procedures: Vec<_> = tx
            .proposal_procedures
            .as_ref()
            .map(|proposals| proposals.iter().collect())
            .unwrap_or_default();

        let votes = tx
            .voting_procedures
            .as_ref()
            .map(Votes::from)
            .unwrap_or_default();

        if let Some(plutus_v1_scripts) = witness_set.plutus_v1_script.as_ref() {
            plutus_v1_scripts.iter().for_each(|script| {
                let script = Script::PlutusV1(script);
                scripts.insert(script.script_hash(), script);
            });
        }

        if let Some(plutus_v2_scripts) = witness_set.plutus_v2_script.as_ref() {
            plutus_v2_scripts.iter().for_each(|script| {
                let script = Script::PlutusV2(script);
                scripts.insert(script.script_hash(), script);
            });
        }

        if let Some(plutus_v3_scripts) = witness_set.plutus_v3_script.as_ref() {
            plutus_v3_scripts.iter().for_each(|script| {
                let script = Script::PlutusV3(script);
                scripts.insert(script.script_hash(), script);
            });
        }

        let mut script_table: BTreeMap<OrderedRedeemer<'a>, Script<'a>> = BTreeMap::new();

        let normalized_redeemers = witness_set
            .redeemer
            .as_ref()
            .map(|redeemers| {
                normalize_redeemers(redeemers.deref())
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
                                    &scripts,
                                    &mut script_table,
                                )
                                .ok_or(TxInfoTranslationError::InvalidRedeemer(ix))?;

                                Ok((purpose, redeemer))
                            })
                            .collect::<Result<
                                Vec<(ScriptPurpose<'a>, Cow<'a, Redeemer>)>,
                                TxInfoTranslationError,
                            >>()
            })
            .transpose()?
            .unwrap_or_default();

        let redeemers = Redeemers::new(normalized_redeemers);

        let datums = witness_set
            .plutus_data
            .as_ref()
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
            signatories,
            redeemers,
            data: datums,
            id: *tx_id,
            votes,
            proposal_procedures,
            current_treasury_amount: tx.treasury_value,
            treasury_donation: tx.donation.map(|donation| donation.into()),
            script_table,
        })
    }

    /// Construct all script contexts for this TxInfo
    pub fn to_script_contexts(&self) -> Vec<(ScriptContext<'_>, &Script<'_>)> {
        self.script_table
            .iter()
            .filter_map(|(redeemer, script)| {
                let script_context = ScriptContext::new(self, redeemer)?;
                Some((script_context, script))
            })
            .collect()
    }

    fn translate_inputs(
        inputs: &'a [TransactionInput],
        utxos: &'a Utxos,
        scripts: &mut BTreeMap<Hash<28>, Script<'a>>,
    ) -> Result<Vec<OutputRef<'a>>, TxInfoTranslationError> {
        inputs
            .iter()
            .sorted()
            .map(|input| {
                let output_ref = utxos
                    .resolve_input(input)
                    .ok_or(TxInfoTranslationError::MissingInput(input.clone().into()))?;

                if let Some(script) = &output_ref.output.script {
                    scripts.insert(script.script_hash(), script.clone());
                };

                Ok(output_ref)
            })
            .collect::<Result<Vec<_>, _>>()
    }
}

#[doc(hidden)]
pub type ScriptPurpose<'a> = ScriptInfo<'a, ()>;

#[doc(hidden)]
#[derive(Clone)]
pub enum ScriptInfo<'a, T: Clone> {
    Minting(PolicyId),
    Spending(&'a TransactionInput, T),
    Rewarding(StakeCredential),
    Certifying(usize, Certificate<'a>),
    Voting(&'a Voter),
    Proposing(usize, &'a Proposal),
}

impl<'a> ScriptPurpose<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn builder(
        redeemer: &Cow<'a, Redeemer>,
        inputs: &[OutputRef<'a>],
        mint: &Mint<'a>,
        withdrawals: &Withdrawals,
        certs: &[Certificate<'a>],
        proposal_procedures: &[&'a Proposal],
        votes: &Votes<'a>,
        scripts: &BTreeMap<Hash<28>, Script<'a>>,
        script_table: &mut BTreeMap<OrderedRedeemer<'a>, Script<'a>>,
    ) -> Option<Self> {
        let index = redeemer.index as usize;
        match redeemer.tag {
            RedeemerTag::Spend => inputs.get(index).and_then(|OutputRef { input, output }| {
                if let Some(StakeCredential::ScriptHash(hash)) = output.address.credential() {
                    let script = scripts.get(&hash);
                    script.map(|script| {
                        script_table.insert(redeemer.clone().into(), script.clone());
                        ScriptPurpose::Spending(input, ())
                    })
                } else {
                    None
                }
            }),
            RedeemerTag::Mint => mint.0.keys().nth(index).copied().and_then(|policy_id| {
                let script = scripts.get(&policy_id);
                script.map(|script| {
                    script_table.insert(redeemer.clone().into(), script.clone());
                    ScriptPurpose::Minting(policy_id)
                })
            }),
            RedeemerTag::Reward => {
                withdrawals
                    .0
                    .keys()
                    .nth(index)
                    .and_then(|stake| match stake.0.payload() {
                        StakePayload::Stake(hash) => Some(ScriptPurpose::Rewarding(
                            StakeCredential::AddrKeyhash(*hash),
                        )),
                        StakePayload::Script(hash) => {
                            let script = scripts.get(hash);
                            script.map(|script| {
                                script_table.insert(redeemer.clone().into(), script.clone());
                                ScriptPurpose::Rewarding(StakeCredential::ScriptHash(*hash))
                            })
                        }
                    })
            }
            RedeemerTag::Cert => certs.get(index).and_then(|certificate| {
                if let Some(StakeCredential::ScriptHash(hash)) = certificate.credential() {
                    let script = scripts.get(&hash);
                    script.map(|script| {
                        script_table.insert(redeemer.clone().into(), script.clone());
                        ScriptPurpose::Certifying(index, certificate.clone())
                    })
                } else {
                    None
                }
            }),
            RedeemerTag::Vote => votes.0.keys().nth(index).and_then(|voter| {
                if let Some(StakeCredential::ScriptHash(hash)) = voter.credential() {
                    let script = scripts.get(&hash);
                    script.map(|script| {
                        script_table.insert(redeemer.clone().into(), script.clone());
                        ScriptPurpose::Voting(voter)
                    })
                } else {
                    None
                }
            }),
            RedeemerTag::Propose => proposal_procedures.get(index).and_then(|proposal| {
                let script_hash = match proposal.gov_action {
                    amaru_kernel::GovAction::ParameterChange(
                        _,
                        _,
                        Nullable::Some(gov_proposal_hash),
                    ) => Some(gov_proposal_hash),
                    amaru_kernel::GovAction::TreasuryWithdrawals(
                        _,
                        Nullable::Some(gov_proposal_hash),
                    ) => Some(gov_proposal_hash),
                    amaru_kernel::GovAction::ParameterChange(..)
                    | amaru_kernel::GovAction::HardForkInitiation(..)
                    | amaru_kernel::GovAction::TreasuryWithdrawals(..)
                    | amaru_kernel::GovAction::NoConfidence(_)
                    | amaru_kernel::GovAction::UpdateCommittee(..)
                    | amaru_kernel::GovAction::NewConstitution(..)
                    | amaru_kernel::GovAction::Information => None,
                };

                if let Some(hash) = script_hash {
                    let script = scripts.get(&hash);
                    script.map(|script| {
                        script_table.insert(redeemer.clone().into(), script.clone());
                        ScriptPurpose::Proposing(index, proposal)
                    })
                } else {
                    None
                }
            }),
        }
    }

    pub fn to_script_info(
        &self,
        data: Option<&'a PlutusData>,
    ) -> ScriptInfo<'a, Option<&'a PlutusData>> {
        match self {
            ScriptInfo::Spending(input, _) => ScriptInfo::Spending(input, data),
            ScriptInfo::Minting(p) => ScriptInfo::Minting(*p),
            ScriptInfo::Rewarding(s) => ScriptInfo::Rewarding(s.clone()),
            ScriptInfo::Certifying(i, c) => ScriptInfo::Certifying(*i, c.clone()),
            ScriptInfo::Voting(v) => ScriptInfo::Voting(v),
            ScriptInfo::Proposing(i, p) => ScriptInfo::Proposing(*i, p),
        }
    }
}

/// A resolved input which includes the output it references.
#[doc(hidden)]
pub struct OutputRef<'a> {
    pub input: &'a TransactionInput,
    pub output: TransactionOutput<'a>,
}

/// A subset of the UTxO set.
///
/// Maps from a `TransactionInput` to a `MemoizedTransactionOutput`
pub struct Utxos(BTreeMap<TransactionInput, MemoizedTransactionOutput>);

impl<'a> Utxos {
    /// Resolve an input to the output it references, returning an [`OutputRef`]
    ///
    ///
    /// Returns `None` when the input cannot be found in the UTxO slice.
    pub fn resolve_input(&'a self, input: &'a TransactionInput) -> Option<OutputRef<'a>> {
        self.0.get(input).map(|utxo| OutputRef {
            input,
            output: utxo.into(),
        })
    }
}

impl Deref for Utxos {
    type Target = BTreeMap<TransactionInput, MemoizedTransactionOutput>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<BTreeMap<TransactionInput, MemoizedTransactionOutput>> for Utxos {
    fn from(value: BTreeMap<TransactionInput, MemoizedTransactionOutput>) -> Self {
        Self(value)
    }
}

/// An interval of time using POSIX time
///
///
/// Time is a difficult, and heavily documented, challenge on Cardano.
/// To maintain deterministic transaction validation,
/// Cardano uses a validity interval which makes a transaction only valid from slot X to slot Y.
///
/// By default, the validity interval is unbounded, meaning the transcation could always be valid.
///
/// One wrinkle that this causes is that while Ouroboros uses slots to handle time, Plutus uses POSIX time.
/// See [`TimeRange::new`] for more information on converting from a validity interval from Ouroboros to a Plutus TimeRange.
#[doc(hidden)]
pub struct TimeRange {
    pub(crate) lower_bound: Option<TimeMs>,
    pub(crate) upper_bound: Option<TimeMs>,
}

impl TimeRange {
    /// Construct a new [`TimeRange`] given a slot interval
    ///
    /// Slot lengths can change based at hard forks, so it is not safe to count slots.
    /// Conversion is handled by the provided `era_history`, which depends on the correct `network` (to determine the `GlobalParamters`)
    ///
    /// There are a few cases that would lead to an `EraHistoryError`:
    /// - `EraHistoryError::PastTimeHorizon`:
    ///   If a bound is too far in the future, we cannot be sure that a hardfork will occur that will change timings.
    ///   The time horizon is the "stability window", which can take up to 3k/f
    /// - `EraHistory::InvalidEraHistory`:
    ///   One of the bounds cannot be found in any era in the `EraHistory`, so we do not know the slot length
    ///   and thus cannot convert to POSIX time. This is typically going to be the result of a user error (incorrect era history)
    pub fn new(
        valid_from_slot: Option<Slot>,
        valid_to_slot: Option<Slot>,
        tip: &Slot,
        era_history: &EraHistory,
        network: NetworkName,
    ) -> Result<Self, EraHistoryError> {
        let parameters: &GlobalParameters = network.into();
        let lower_bound = valid_from_slot
            .map(|slot| era_history.slot_to_posix_time(slot, *tip, parameters.system_start.into()))
            .transpose()?;
        let upper_bound = valid_to_slot
            .map(|slot| era_history.slot_to_posix_time(slot, *tip, parameters.system_start.into()))
            .transpose()?;

        Ok(Self {
            lower_bound,
            upper_bound,
        })
    }
}

/// An identifier for a currency in a [`Value`]
///
/// See [`Value`] for more
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CurrencySymbol {
    Lovelace,
    Native(Hash<28>),
}

impl From<Hash<28>> for CurrencySymbol {
    fn from(value: Hash<28>) -> Self {
        Self::Native(value)
    }
}

/// A representation of `Value` used in Plutus
///
/// The ledger's `Value` contains both a `Coin` and, optionally, a `Multiasset`.
/// In Plutus, this is simply a single map, with an empty bytestring representing lovelace
#[doc(hidden)]
#[derive(Clone)]
pub struct Value<'a>(pub BTreeMap<CurrencySymbol, BTreeMap<Cow<'a, AssetName>, u64>>);

impl<'a> From<&'a amaru_kernel::Value> for Value<'a> {
    fn from(value: &'a amaru_kernel::Value) -> Self {
        let assets = match value {
            amaru_kernel::Value::Coin(coin) => BTreeMap::from([(
                CurrencySymbol::Lovelace,
                BTreeMap::from([(Cow::Owned(Bytes::from(vec![])), *coin)]),
            )]),
            amaru_kernel::Value::Multiasset(coin, multiasset) => {
                let mut map = BTreeMap::new();
                map.insert(
                    CurrencySymbol::Lovelace,
                    BTreeMap::from([(Cow::Owned(Bytes::from(vec![])), *coin)]),
                );
                multiasset.iter().for_each(|(policy_id, asset_bundle)| {
                    map.insert(
                        CurrencySymbol::Native(*policy_id),
                        asset_bundle
                            .iter()
                            .map(|(asset_name, amount)| (Cow::Borrowed(asset_name), amount.into()))
                            .collect(),
                    );
                });

                map
            }
        };

        Self(assets)
    }
}

impl<'a> From<amaru_kernel::Value> for Value<'a> {
    fn from(value: amaru_kernel::Value) -> Self {
        let assets = match value {
            amaru_kernel::Value::Coin(coin) => BTreeMap::from([(
                CurrencySymbol::Lovelace,
                BTreeMap::from([(Cow::Owned(Bytes::from(vec![])), coin)]),
            )]),
            amaru_kernel::Value::Multiasset(coin, multiasset) => {
                let mut map = BTreeMap::new();
                map.insert(
                    CurrencySymbol::Lovelace,
                    BTreeMap::from([(Cow::Owned(Bytes::from(vec![])), coin)]),
                );
                multiasset
                    .into_iter()
                    .for_each(|(policy_id, asset_bundle)| {
                        map.insert(
                            CurrencySymbol::Native(policy_id),
                            asset_bundle
                                .into_iter()
                                .map(|(asset_name, amount)| (Cow::Owned(asset_name), amount.into()))
                                .collect(),
                        );
                    });

                map
            }
        };

        Self(assets)
    }
}

impl From<Lovelace> for Value<'_> {
    fn from(coin: Lovelace) -> Self {
        Self(BTreeMap::from([(
            CurrencySymbol::Lovelace,
            BTreeMap::from([(Cow::Owned(Bytes::from(vec![])), coin)]),
        )]))
    }
}

#[doc(hidden)]
#[derive(Debug, Error)]
pub enum AlonzoValueError {
    #[error("invalid quantity: {0}")]
    InvalidQuantity(u64),
}
impl<'a> TryFrom<&'a AlonzoValue> for Value<'a> {
    type Error = AlonzoValueError;

    fn try_from(value: &'a AlonzoValue) -> Result<Self, Self::Error> {
        let from_tokens = |tokens: &'a KeyValuePairs<AssetName, Lovelace>| {
            (*tokens)
                .iter()
                .map(|(asset_name, quantity)| {
                    if *quantity > 0 {
                        Ok((Cow::Borrowed(asset_name), *quantity))
                    } else {
                        Err(AlonzoValueError::InvalidQuantity(*quantity))
                    }
                })
                .collect::<Result<BTreeMap<_, _>, Self::Error>>()
        };

        match value {
            AlonzoValue::Coin(coin) => Ok((*coin).into()),
            AlonzoValue::Multiasset(coin, multiasset) if multiasset.is_empty() => {
                Ok((*coin).into())
            }
            AlonzoValue::Multiasset(coin, multiasset) => {
                let mut map = BTreeMap::new();
                map.insert(
                    CurrencySymbol::Lovelace,
                    BTreeMap::from([(Cow::Owned(Bytes::from(vec![])), *coin)]),
                );

                for (policy_id, tokens) in multiasset.deref() {
                    map.insert(CurrencySymbol::Native(*policy_id), from_tokens(tokens)?);
                }

                Ok(Self(map))
            }
        }
    }
}

impl Value<'_> {
    pub fn ada(&self) -> Option<u64> {
        self.0
            .get(&CurrencySymbol::Lovelace)
            .and_then(|asset_bundle| {
                asset_bundle.iter().find_map(|(name, amount)| {
                    if name.is_empty() && amount != &0 {
                        Some(*amount)
                    } else {
                        None
                    }
                })
            })
    }
}

#[doc(hidden)]
#[derive(Clone)]
pub enum Script<'a> {
    Native(&'a NativeScript),
    PlutusV1(&'a PlutusScript<1>),
    PlutusV2(&'a PlutusScript<2>),
    PlutusV3(&'a PlutusScript<3>),
}

impl Script<'_> {
    pub fn to_bytes(&self) -> Result<Vec<u8>, minicbor::decode::Error> {
        fn decode_cbor_bytes(cbor: &[u8]) -> Result<Vec<u8>, minicbor::decode::Error> {
            minicbor::decode::Decoder::new(cbor)
                .bytes()
                .map(|b| b.to_vec())
        }

        match self {
            Script::PlutusV1(s) => decode_cbor_bytes(s.0.as_ref()),
            Script::PlutusV2(s) => decode_cbor_bytes(s.0.as_ref()),
            Script::PlutusV3(s) => decode_cbor_bytes(s.0.as_ref()),
            Script::Native(_) => unreachable!("a redeemer should never point to a native_script"),
        }
    }
}

impl<'a> From<&'a CborWrap<MintedScriptRef<'a>>> for Script<'a> {
    fn from(value: &'a CborWrap<MintedScriptRef<'a>>) -> Self {
        match &value.0 {
            PseudoScript::NativeScript(script) => Script::Native(script.deref()),
            PseudoScript::PlutusV1Script(script) => Script::PlutusV1(script),
            PseudoScript::PlutusV2Script(script) => Script::PlutusV2(script),
            PseudoScript::PlutusV3Script(script) => Script::PlutusV3(script),
        }
    }
}

impl<'a> From<&'a MemoizedScript> for Script<'a> {
    fn from(value: &'a MemoizedScript) -> Self {
        match value {
            PseudoScript::NativeScript(script) => Script::Native(script.as_ref()),
            PseudoScript::PlutusV1Script(script) => Script::PlutusV1(script),
            PseudoScript::PlutusV2Script(script) => Script::PlutusV2(script),
            PseudoScript::PlutusV3Script(script) => Script::PlutusV3(script),
        }
    }
}

impl<'a> HasScriptHash for Script<'a> {
    fn script_hash(&self) -> amaru_kernel::ScriptHash {
        match self {
            Script::Native(native_script) => native_script.compute_hash(),
            Script::PlutusV1(plutus_script) => plutus_script.compute_hash(),
            Script::PlutusV2(plutus_script) => plutus_script.compute_hash(),
            Script::PlutusV3(plutus_script) => plutus_script.compute_hash(),
        }
    }
}

#[doc(hidden)]
#[derive(Clone)]
pub enum DatumOption<'a> {
    None,
    Hash(&'a DatumHash),
    Inline(&'a PlutusData),
}

impl<'a> From<&'a MemoizedDatum> for DatumOption<'a> {
    fn from(value: &'a MemoizedDatum) -> Self {
        match value {
            MemoizedDatum::None => Self::None,
            MemoizedDatum::Hash(hash) => Self::Hash(hash),
            MemoizedDatum::Inline(data) => Self::Inline(data.as_ref()),
        }
    }
}

impl<'a> From<Option<&'a MintedDatumOption<'a>>> for DatumOption<'a> {
    fn from(value: Option<&'a MintedDatumOption<'a>>) -> Self {
        match value {
            None => Self::None,
            Some(MintedDatumOption::Hash(hash)) => Self::Hash(hash),
            Some(MintedDatumOption::Data(data)) => Self::Inline(data.deref()),
        }
    }
}

impl<'a> From<Option<&'a Hash<32>>> for DatumOption<'a> {
    fn from(value: Option<&'a Hash<32>>) -> Self {
        match value {
            Some(hash) => Self::Hash(hash),
            None => Self::None,
        }
    }
}

#[doc(hidden)]
#[derive(Clone)]
pub struct TransactionOutput<'a> {
    pub is_legacy: bool,
    pub address: Cow<'a, Address>,
    pub value: Value<'a>,
    pub datum: DatumOption<'a>,
    pub script: Option<Script<'a>>,
}

impl<'a> From<&'a MemoizedTransactionOutput> for TransactionOutput<'a> {
    fn from(output: &'a MemoizedTransactionOutput) -> Self {
        Self {
            is_legacy: output.is_legacy,
            address: Cow::Borrowed(&output.address),
            value: (&output.value).into(),
            datum: (&output.datum).into(),
            script: output.script.as_ref().map(Script::from),
        }
    }
}

#[doc(hidden)]
#[derive(Debug, Error)]
pub enum TransactionOutputError {
    #[error("invalid address: {0}")]
    InvalidAddress(#[from] AddressError),
    #[error("invalid value: {0}")]
    InvalidValue(#[from] AlonzoValueError),
}

impl<'a> TryFrom<&'a MintedTransactionOutput<'a>> for TransactionOutput<'a> {
    type Error = TransactionOutputError;

    fn try_from(
        output: &'a MintedTransactionOutput<'a>,
    ) -> Result<TransactionOutput<'a>, Self::Error> {
        match output {
            MintedTransactionOutput::Legacy(output) => Ok(TransactionOutput {
                is_legacy: true,
                address: Cow::Owned(Address::from_bytes(&output.address)?),
                value: (&output.amount).try_into()?,
                datum: output.datum_hash.as_ref().into(),
                script: None,
            }),
            MintedTransactionOutput::PostAlonzo(output) => Ok(TransactionOutput {
                is_legacy: false,
                address: Cow::Owned(Address::from_bytes(&output.address)?),
                value: (&output.value).into(),
                script: output.script_ref.as_ref().map(Script::from),
                datum: output.datum_option.as_ref().into(),
            }),
        }
    }
}

#[doc(hidden)]
#[derive(Debug, Default)]
pub struct Mint<'a>(pub BTreeMap<Hash<28>, BTreeMap<Cow<'a, AssetName>, i64>>);

impl<'a> From<&'a amaru_kernel::Mint> for Mint<'a> {
    fn from(value: &'a amaru_kernel::Mint) -> Self {
        let mints = value
            .iter()
            .map(|(policy, multiasset)| {
                (
                    *policy,
                    multiasset
                        .iter()
                        .map(|(asset_name, amount)| (Cow::Borrowed(asset_name), (*amount).into()))
                        .collect(),
                )
            })
            .collect();

        Self(mints)
    }
}

#[doc(hidden)]
#[derive(Default)]
pub struct RequiredSigners(pub BTreeSet<AddrKeyhash>);

impl<'a> From<&'a amaru_kernel::RequiredSigners> for RequiredSigners {
    fn from(value: &'a amaru_kernel::RequiredSigners) -> Self {
        Self(value.iter().copied().collect())
    }
}

#[doc(hidden)]
#[derive(Default)]
pub struct Withdrawals(pub BTreeMap<StakeAddress, Lovelace>);

#[doc(hidden)]
#[derive(Clone)]
pub struct Certificate<'a> {
    // There is a bug in conway protocol 9 that means we have to change our serialization logic depending on the protocol version
    protocol_verison: ProtocolVersion,
    certificate: &'a PallasCertificate,
}

impl<'a> Deref for Certificate<'a> {
    type Target = PallasCertificate;

    fn deref(&self) -> &Self::Target {
        self.certificate
    }
}

#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct StakeAddress(amaru_kernel::StakeAddress);

impl From<StakeAddress> for amaru_kernel::StakeAddress {
    fn from(value: StakeAddress) -> Self {
        value.0
    }
}

// Reference for this ordering is
// https://github.com/aiken-lang/aiken/blob/a8c032935dbaf4a1140e9d8be5c270acd32c9e8c/crates/uplc/src/tx/script_context.rs#L1112
impl Ord for StakeAddress {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        fn network_tag(network: Network) -> u8 {
            match network {
                Network::Testnet => 0,
                Network::Mainnet => 1,
                Network::Other(tag) => tag,
            }
        }

        if self.0.network() != other.0.network() {
            return network_tag(self.0.network()).cmp(&network_tag(other.0.network()));
        }

        match (self.0.payload(), other.0.payload()) {
            (StakePayload::Script(..), StakePayload::Stake(..)) => Ordering::Less,
            (StakePayload::Stake(..), StakePayload::Script(..)) => Ordering::Greater,
            (StakePayload::Script(hash_a), StakePayload::Script(hash_b)) => hash_a.cmp(hash_b),
            (StakePayload::Stake(hash_a), StakePayload::Stake(hash_b)) => hash_a.cmp(hash_b),
        }
    }
}

impl PartialOrd for StakeAddress {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for StakeAddress {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for StakeAddress {}

#[doc(hidden)]
#[derive(Debug, Error)]
pub enum WithdrawalError {
    #[error("invalid reward account: {0}")]
    InvalidRewardAccount(#[from] AddressError),
    #[error("invalid address type: {0}")]
    InvalidAddressType(Address),
}

impl TryFrom<&NonEmptyKeyValuePairs<RewardAccount, Lovelace>> for Withdrawals {
    type Error = WithdrawalError;

    fn try_from(
        value: &NonEmptyKeyValuePairs<RewardAccount, Lovelace>,
    ) -> Result<Self, Self::Error> {
        let withdrawals = value
            .iter()
            .map(|(reward_account, coin)| {
                let address = Address::from_bytes(reward_account)?;

                if let Address::Stake(reward_account) = address {
                    Ok((StakeAddress(reward_account), *coin))
                } else {
                    Err(WithdrawalError::InvalidAddressType(address))
                }
            })
            .collect::<Result<BTreeMap<_, _>, WithdrawalError>>()?;

        Ok(Self(withdrawals))
    }
}

#[doc(hidden)]
#[derive(Default)]
pub struct Datums<'a>(pub BTreeMap<DatumHash, &'a PlutusData>);

impl<'a> From<&'a NonEmptySet<KeepRaw<'_, PlutusData>>> for Datums<'a> {
    fn from(plutus_data: &'a NonEmptySet<KeepRaw<'_, PlutusData>>) -> Self {
        Self(
            plutus_data
                .iter()
                .map(|data| (data.compute_hash(), data.deref()))
                .collect(),
        )
    }
}

#[doc(hidden)]
pub struct Redeemers<'a, T>(pub Vec<(T, Cow<'a, Redeemer>)>);

impl<'a, T> Redeemers<'a, T> {
    pub fn new(redeemers: Vec<(T, Cow<'a, Redeemer>)>) -> Self {
        Self(
            redeemers
                .into_iter()
                .sorted_by(|(_, redeemer_a), (_, redeemer_b)| {
                    Self::sort_redeemers(redeemer_a, redeemer_b)
                })
                .collect(),
        )
    }

    fn sort_redeemers(a: &Redeemer, b: &Redeemer) -> Ordering {
        match a.tag.as_index().cmp(&b.tag.as_index()) {
            by_tag @ Ordering::Less | by_tag @ Ordering::Greater => by_tag,
            Ordering::Equal => a.index.cmp(&b.index),
        }
    }
}

#[doc(hidden)]
#[derive(Default)]
pub struct Votes<'a>(pub BTreeMap<&'a Voter, BTreeMap<ProposalIdAdapter<'a>, &'a Vote>>);

impl<'a> From<&'a VotingProcedures> for Votes<'a> {
    fn from(voting_procedures: &'a VotingProcedures) -> Self {
        Self(
            voting_procedures
                .iter()
                .map(|(voter, votes)| {
                    (
                        voter,
                        votes
                            .iter()
                            .map(|(proposal, procedure)| (proposal.into(), &procedure.vote))
                            .collect(),
                    )
                })
                .collect(),
        )
    }
}

#[cfg(test)]
pub mod test_vectors {
    use std::{collections::BTreeMap, sync::LazyLock};

    use amaru_kernel::{
        Address, MemoizedDatum, MemoizedTransactionOutput, TransactionInput, include_json,
        serde_utils::hex_to_bytes,
    };
    use serde::Deserialize;

    #[derive(Deserialize)]
    pub struct TestVector {
        pub meta: TestMeta,
        pub input: TestInput,
        pub expectations: TestExpectations,
    }

    #[derive(Debug, Deserialize)]
    pub struct TestMeta {
        pub title: String,
        pub description: String,
        pub plutus_version: u8,
    }

    #[derive(Deserialize)]
    pub struct TestInput {
        #[serde(rename = "transaction", deserialize_with = "hex_to_bytes")]
        pub transaction_bytes: Vec<u8>,
        #[serde(deserialize_with = "deserialize_utxo")]
        pub utxo: BTreeMap<TransactionInput, MemoizedTransactionOutput>,
    }

    #[derive(Deserialize)]
    pub struct TestExpectations {
        pub script_context: String,
    }

    pub struct MemoizedTransactionOutputWrapper(pub MemoizedTransactionOutput);

    fn deserialize_utxo<'de, D>(
        deserializer: D,
    ) -> Result<BTreeMap<TransactionInput, MemoizedTransactionOutput>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct UtxoVisitor;

        impl<'a> serde::de::Visitor<'a> for UtxoVisitor {
            type Value = BTreeMap<TransactionInput, MemoizedTransactionOutput>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("UTxOs")
            }

            fn visit_seq<V>(self, mut seq: V) -> Result<Self::Value, V::Error>
            where
                V: serde::de::SeqAccess<'a>,
            {
                let mut utxo_map = BTreeMap::new();

                while let Some(entry) = seq.next_element::<UtxoEntryHelper>()? {
                    let tx_id_bytes =
                        hex::decode(&entry.transaction.id).map_err(serde::de::Error::custom)?;

                    let input = TransactionInput {
                        transaction_id: tx_id_bytes.as_slice().into(),
                        index: entry.index,
                    };

                    utxo_map.insert(input, entry.output.0);
                }

                Ok(utxo_map)
            }
        }

        #[derive(Deserialize)]
        struct UtxoEntryHelper {
            transaction: TransactionIdHelper,
            index: u64,
            #[serde(flatten)]
            output: MemoizedTransactionOutputWrapper,
        }

        #[derive(Deserialize)]
        struct TransactionIdHelper {
            id: String,
        }

        deserializer.deserialize_seq(UtxoVisitor)
    }

    impl<'a> serde::Deserialize<'a> for MemoizedTransactionOutputWrapper {
        fn deserialize<D: serde::Deserializer<'a>>(deserializer: D) -> Result<Self, D::Error> {
            #[derive(serde::Deserialize)]
            #[serde(field_identifier, rename_all = "snake_case")]
            enum Field {
                Address,
                Value,
                DatumHash,
                Datum,
                Script,
            }

            const FIELDS: &[&str] = &["address", "value", "datum", "datum_hash", "script"];

            struct TransactionOutputVisitor;

            impl<'a> serde::de::Visitor<'a> for TransactionOutputVisitor {
                type Value = MemoizedTransactionOutputWrapper;

                fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    formatter.write_str("TransationOutput")
                }

                fn visit_map<V>(
                    self,
                    mut map: V,
                ) -> Result<MemoizedTransactionOutputWrapper, V::Error>
                where
                    V: serde::de::MapAccess<'a>,
                {
                    let mut address = None;
                    let mut value = None;
                    let mut datum = MemoizedDatum::None;
                    let script = None;

                    let assert_only_datum_or_hash = |datum: &MemoizedDatum| {
                        if datum != &MemoizedDatum::None {
                            return Err("cannot have both datum_hash and datum".to_string());
                        }

                        Ok(())
                    };

                    while let Some(key) = map.next_key()? {
                        match key {
                            Field::Address => {
                                let string: String = map.next_value()?;
                                let bytes =
                                    hex::decode(string).map_err(serde::de::Error::custom)?;
                                address = Some(
                                    Address::from_bytes(&bytes)
                                        .map_err(serde::de::Error::custom)?,
                                );
                            }
                            Field::Value => {
                                let helper: BTreeMap<String, BTreeMap<String, u64>> =
                                    map.next_value()?;
                                value = Some(amaru_kernel::Value::Coin(
                                    *helper
                                        .get("ada")
                                        .ok_or_else(|| serde::de::Error::missing_field("ada"))?
                                        .get("lovelace")
                                        .ok_or_else(|| {
                                            serde::de::Error::missing_field("lovelace")
                                        })?,
                                ));
                            }
                            Field::Datum => {
                                assert_only_datum_or_hash(&datum)
                                    .map_err(serde::de::Error::custom)?;
                                let string: String = map.next_value()?;
                                datum = MemoizedDatum::Inline(
                                    string.try_into().map_err(serde::de::Error::custom)?,
                                );
                            }
                            Field::DatumHash => {
                                assert_only_datum_or_hash(&datum)
                                    .map_err(serde::de::Error::custom)?;
                                let string: String = map.next_value()?;
                                let bytes: Vec<u8> =
                                    hex::decode(string).map_err(serde::de::Error::custom)?;
                                datum = MemoizedDatum::Hash(bytes.as_slice().into())
                            }
                            Field::Script => {
                                unimplemented!("script in UTxO not yet supported");
                            }
                        }
                    }

                    Ok(MemoizedTransactionOutputWrapper(
                        MemoizedTransactionOutput {
                            is_legacy: false,
                            address: address
                                .ok_or_else(|| serde::de::Error::missing_field("address"))?,
                            value: value.ok_or_else(|| serde::de::Error::missing_field("value"))?,
                            datum,
                            script,
                        },
                    ))
                }
            }

            deserializer.deserialize_struct("TransationOutput", FIELDS, TransactionOutputVisitor)
        }
    }

    static TEST_VECTORS: LazyLock<Vec<TestVector>> =
        LazyLock::new(|| include_json!("script-context-fixtures.json"));

    pub fn get_test_vectors(version: u8) -> Vec<&'static TestVector> {
        TEST_VECTORS
            .iter()
            .filter(|vector| vector.meta.plutus_version == version)
            .collect()
    }

    pub fn get_test_vector(title: &str, ver: u8) -> &'static TestVector {
        get_test_vectors(ver)
            .iter()
            .find(|vector| vector.meta.title == title)
            .unwrap_or_else(|| panic!("Test case not found: {title}"))
    }
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{ShelleyAddress, ShelleyDelegationPart};
    use proptest::{
        prelude::{Just, Strategy, any, prop},
        prop_assert, prop_oneof, proptest,
    };

    use crate::ToPlutusData;

    use super::*;

    fn network_strategy() -> impl Strategy<Value = Network> {
        prop_oneof![
            Just(Network::Testnet),
            Just(Network::Mainnet),
            any::<u8>().prop_map(Network::from),
        ]
    }

    fn stake_address_strategy() -> impl Strategy<Value = StakeAddress> {
        (prop::bool::ANY, any::<[u8; 28]>(), network_strategy()).prop_map(
            |(is_script, hash_bytes, network)| {
                let delegation: ShelleyDelegationPart = if is_script {
                    ShelleyDelegationPart::Script(hash_bytes.into())
                } else {
                    ShelleyDelegationPart::Key(hash_bytes.into())
                };

                // It is not possible to construct a StakeAddress from the parts of it in Pallas.
                // Instead, we're constructing a ShelelyAddress and then converting it to a StakeAddress.
                // The conversion uses the ShelleyDelegationPart, so the ShelleyPaymentPart is totally arbitrary here
                let shelley_addr = ShelleyAddress::new(
                    network,
                    amaru_kernel::ShelleyPaymentPart::Key(hash_bytes.into()),
                    delegation,
                );

                StakeAddress(shelley_addr.try_into().unwrap())
            },
        )
    }

    #[test]
    fn proptest_stake_address_ordering() {
        proptest!(|(addresses in prop::collection::vec(stake_address_strategy(), 20..100))| {
            let mut sorted = addresses.clone();
            sorted.sort();


            for window in sorted.windows(2) {
                let a = &window[0];
                let b = &window[1];

                fn network_tag(network: Network) -> u8 {
                    match network {
                        Network::Testnet => 0,
                        Network::Mainnet => 1,
                        Network::Other(tag) => tag,
                    }
                }

                let net_a = a.0.network();
                let net_b = b.0.network();


                // We sort by network first (testnet, mainnet, other by tag)
                if net_a != net_b {
                    prop_assert!(
                        network_tag(net_a) < network_tag(net_b),
                        "Network ordering violated: {:?} should be < {:?}",
                        network_tag(net_a),
                        network_tag(net_b)
                    );
                } else {
                    match (a.0.payload(), b.0.payload()) {
                        // Script < Stake
                        (StakePayload::Script(_), StakePayload::Stake(_)) => {
                            // This is correct
                        }
                        (StakePayload::Stake(_), StakePayload::Script(_)) => {
                            prop_assert!(false, "Payload type ordering violated: Stake should not come before Script");
                        }
                        // Same payload compare bytes
                        (StakePayload::Script(h1), StakePayload::Script(h2)) => {
                            prop_assert!(
                                h1 <= h2,
                                "Script hash ordering violated: {:?} should be <= {:?}",
                                h1, h2
                            );
                        }
                        (StakePayload::Stake(h1), StakePayload::Stake(h2)) => {
                            prop_assert!(
                                h1 <= h2,
                                "Stake hash ordering violated: {:?} should be <= {:?}",
                                h1, h2
                            );
                        }
                    }
                }
            }
        });
    }

    #[test]
    fn proptest_value_zero_ada_excluded_in_v3() {
        proptest!(|(policies in prop::collection::vec(any::<[u8; 28]>(), 1..5))| {
            let mut value_map = BTreeMap::new();

            // We should be excluding ADA values with a quantity of zero in Plutus V3
            let mut ada_map = BTreeMap::new();
            ada_map.insert(Cow::Owned(Bytes::from(vec![])), 0u64);
            value_map.insert(CurrencySymbol::Lovelace, ada_map);

            for policy_bytes in policies {
                let mut asset_map = BTreeMap::new();
                asset_map.insert(Cow::Owned(Bytes::from(vec![1])), 100);
                value_map.insert(CurrencySymbol::Native(policy_bytes.into()), asset_map);
            }

            let value = Value(value_map);
            let plutus_data = <Value<'_> as ToPlutusData<3>>::to_plutus_data(&value)?;

            #[allow(clippy::wildcard_enum_match_arm)]
            match plutus_data {
                PlutusData::Map(KeyValuePairs::Def(pairs)) => {
                    let has_ada = pairs.iter().any(|(key, _)| {
                        matches!(key, PlutusData::BoundedBytes(b) if b.is_empty())
                    });

                    prop_assert!(!has_ada,
                        "V3 Value should exclude ADA entry when amount is zero. Found {} pairs",
                        pairs.len());

                    prop_assert!(!pairs.is_empty(),
                        "Should still have non-ADA assets in the map");
                }
                other => {
                    prop_assert!(false, "Value should encode as Map, got: {:?}", other);
                }
            }
        });
    }

    #[test]
    fn proptest_value_nonzero_ada_included_in_v3() {
        proptest!(|(
            ada_amount in 1u64..,
            policies in prop::collection::vec(any::<[u8; 28]>(), 1..5)
        )| {
            let mut value_map = BTreeMap::new();

            let mut ada_map = BTreeMap::new();
            ada_map.insert(Cow::Owned(Bytes::from(vec![])), ada_amount);
            value_map.insert(CurrencySymbol::Lovelace, ada_map);

            for policy_bytes in policies {
                let mut asset_map = BTreeMap::new();
                asset_map.insert(Cow::Owned(Bytes::from(vec![1])), 100);
                value_map.insert(CurrencySymbol::Native(policy_bytes.into()), asset_map);
            }

            let value = Value(value_map);
            let plutus_data = <Value<'_> as ToPlutusData<3>>::to_plutus_data(&value)?;

            #[allow(clippy::wildcard_enum_match_arm)]
            match plutus_data {
                PlutusData::Map(KeyValuePairs::Def(pairs)) => {
                    let ada_entry = pairs.iter().find(|(key, _)| {
                        matches!(key, PlutusData::BoundedBytes(b) if b.is_empty())
                    });

                    prop_assert!(ada_entry.is_some(),
                        "V3 Value should include ADA entry when amount is non-zero");
                }
                other => {
                    prop_assert!(false, "Value should encode as Map, got: {:?}", other);
                }
            }
        });
    }
}
