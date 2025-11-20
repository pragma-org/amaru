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

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
};

use amaru_kernel::{
    AddrKeyhash, Address, AddressError, AssetName, Certificate, ComputeHash, DatumHash, EraHistory,
    Hash, KeepRaw, Lovelace, MintedTransactionBody, MintedWitnessSet, Network,
    NonEmptyKeyValuePairs, NonEmptySet, PlutusData, PolicyId, Proposal, ProposalIdAdapter,
    Redeemer, RewardAccount, ScriptPurpose as RedeemerTag, Slot, StakeCredential, StakePayload,
    TransactionId, TransactionInput, TransactionInputAdapter, Vote, Voter, VotingProcedures,
    network::NetworkName, normalize_redeemers, protocol_parameters::GlobalParameters,
};
use amaru_slot_arithmetic::{EraHistoryError, TimeMs};
use itertools::Itertools;
use thiserror::Error;

use crate::script_context::Utxos;

pub mod output;
pub use output::*;

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

/// An opaque type that represents the `TxInfo` field used in a [`ScriptContext`].
///
/// `TxInfo` is an in-memory representation of a Cardano transaction used in Plutus scripts.
///
/// Notably, it is not an exact mapping of the transaction on the ledger.
/// For example, bootstrap addresses are skipped in the inputs, reference inputs, and outputs.
#[derive(Debug)]
pub struct TxInfo {
    pub(crate) inputs: Vec<OutputRef>,
    pub(crate) reference_inputs: Vec<OutputRef>,
    pub(crate) outputs: Vec<TransactionOutput>,
    pub(crate) fee: Lovelace,
    pub(crate) mint: Mint,
    pub(crate) certificates: Vec<Certificate>,
    pub(crate) withdrawals: Withdrawals,
    pub(crate) valid_range: TimeRange,
    pub(crate) signatories: RequiredSigners,
    pub(crate) redeemers: Redeemers<ScriptPurpose>,
    pub(crate) data: Datums,
    pub(crate) id: TransactionId,
    pub(crate) votes: Votes,
    pub(crate) proposal_procedures: Vec<Proposal>,
    pub(crate) current_treasury_amount: Option<Lovelace>,
    pub(crate) treasury_donation: Option<Lovelace>,
}

impl TxInfo {
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
    pub fn new(
        tx: MintedTransactionBody<'_>,
        witness_set: MintedWitnessSet<'_>,
        tx_id: &Hash<32>,
        utxos: &Utxos,
        slot: &Slot,
        network: NetworkName,
        era_history: &EraHistory,
    ) -> Result<Self, TxInfoTranslationError> {
        let inputs = Self::translate_inputs(tx.inputs.to_vec(), utxos)?;
        let reference_inputs = tx
            .reference_inputs
            .map(|ref_inputs| Self::translate_inputs(ref_inputs.to_vec(), utxos))
            .transpose()?
            .unwrap_or_default();

        let outputs = tx
            .outputs
            .into_iter()
            .map(TransactionOutput::try_from)
            .collect::<Result<Vec<_>, _>>()?;

        let mint = tx.mint.map(Mint::from).unwrap_or_default();

        let certificates: Vec<Certificate> = tx
            .certificates
            .map(|certificates| certificates.to_vec())
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
            .map(|proposals| proposals.to_vec())
            .unwrap_or_default();

        let votes = tx.voting_procedures.map(Votes::from).unwrap_or_default();

        let redeemers = Redeemers(
            witness_set
                .redeemer
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
                            .ok_or(TxInfoTranslationError::InvalidRedeemer(ix))?;

                            Ok((purpose, redeemer))
                        })
                        .collect::<Result<Vec<(ScriptPurpose, Redeemer)>, TxInfoTranslationError>>()
                })
                .transpose()?
                .unwrap_or_default(),
        );

        let data = witness_set
            .plutus_data
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
            data,
            id: *tx_id,
            votes,
            proposal_procedures,
            current_treasury_amount: tx.treasury_value,
            treasury_donation: tx.donation.map(|donation| donation.into()),
        })
    }

    pub fn empty(tx_id: &TransactionId) -> Self {
        TxInfo {
            id: *tx_id,
            inputs: Default::default(),
            reference_inputs: Default::default(),
            outputs: Default::default(),
            fee: Default::default(),
            mint: Default::default(),
            certificates: Default::default(),
            withdrawals: Default::default(),
            valid_range: Default::default(),
            signatories: Default::default(),
            redeemers: Default::default(),
            data: Default::default(),
            votes: Default::default(),
            proposal_procedures: Default::default(),
            current_treasury_amount: Default::default(),
            treasury_donation: Default::default(),
        }
    }

    fn translate_inputs(
        inputs: Vec<TransactionInput>,
        utxos: &Utxos,
    ) -> Result<Vec<OutputRef>, TxInfoTranslationError> {
        inputs
            .into_iter()
            .sorted()
            .map(|input| {
                utxos
                    .resolve_input(&input)
                    .ok_or(TxInfoTranslationError::MissingInput(input.clone().into()))
            })
            .collect::<Result<Vec<_>, _>>()
    }
}

// ScriptPupose
// ----------------------------------------------------------------------------

#[doc(hidden)]
pub type ScriptPurpose = ScriptInfo<()>;

#[doc(hidden)]
#[derive(Clone, Debug)]
pub enum ScriptInfo<T: Clone> {
    Minting(PolicyId),
    Spending(TransactionInput, T),
    Rewarding(StakeCredential),
    Certifying(usize, Certificate),
    Voting(Voter),
    Proposing(usize, Proposal),
}

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
                .map(|voter| ScriptPurpose::Voting(voter.clone())),
            RedeemerTag::Propose => proposal_procedures
                .get(index)
                .map(|proposal| ScriptPurpose::Proposing(index, proposal.clone())),
        }
    }

    pub fn to_script_info(self, data: Option<PlutusData>) -> ScriptInfo<Option<PlutusData>> {
        match self {
            ScriptInfo::Spending(input, _) => ScriptInfo::Spending(input, data),
            ScriptInfo::Minting(policy) => ScriptInfo::Minting(policy),
            ScriptInfo::Rewarding(stake_credential) => ScriptInfo::Rewarding(stake_credential),
            ScriptInfo::Certifying(ix, certificate) => ScriptInfo::Certifying(ix, certificate),
            ScriptInfo::Voting(voter) => ScriptInfo::Voting(voter),
            ScriptInfo::Proposing(ix, proposal) => ScriptInfo::Proposing(ix, proposal),
        }
    }
}

// Mint
// ----------------------------------------------------------------------------
#[doc(hidden)]
#[derive(Debug, Default)]
pub struct Mint(pub BTreeMap<Hash<28>, BTreeMap<AssetName, i64>>);

impl From<amaru_kernel::Mint> for Mint {
    fn from(value: amaru_kernel::Mint) -> Self {
        let mints = value
            .into_iter()
            .map(|(policy, multiasset)| {
                (
                    policy,
                    multiasset
                        .into_iter()
                        .map(|(asset_name, amount)| (asset_name, amount.into()))
                        .collect(),
                )
            })
            .collect();

        Self(mints)
    }
}

// Withdrawals
// ----------------------------------------------------------------------------

#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct StakeAddress(pub(crate) amaru_kernel::StakeAddress);

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
#[derive(Default, Debug)]
pub struct Withdrawals(pub BTreeMap<StakeAddress, Lovelace>);

// TimeRange
// ----------------------------------------------------------------------------

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
#[derive(Debug, Default)]
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

// RequiredSigners
// ----------------------------------------------------------------------------

#[doc(hidden)]
#[derive(Default, Debug)]
pub struct RequiredSigners(pub BTreeSet<AddrKeyhash>);

impl From<&amaru_kernel::RequiredSigners> for RequiredSigners {
    fn from(value: &amaru_kernel::RequiredSigners) -> Self {
        Self(value.iter().copied().collect())
    }
}

// Datums
// ----------------------------------------------------------------------------

#[doc(hidden)]
#[derive(Default, Debug)]
pub struct Datums(pub BTreeMap<DatumHash, PlutusData>);

impl From<NonEmptySet<KeepRaw<'_, PlutusData>>> for Datums {
    fn from(plutus_data: NonEmptySet<KeepRaw<'_, PlutusData>>) -> Self {
        Self(
            plutus_data
                .to_vec()
                .into_iter()
                .map(|data| (data.compute_hash(), data.unwrap()))
                .collect(),
        )
    }
}

// Redeemers
// ----------------------------------------------------------------------------

#[doc(hidden)]
#[derive(Debug)]
// FIXME: This should probably be a BTreeMap
pub struct Redeemers<T>(pub Vec<(T, Redeemer)>);

impl<T> Default for Redeemers<T> {
    fn default() -> Self {
        Self(Default::default())
    }
}

// Votes
// ----------------------------------------------------------------------------

#[doc(hidden)]
#[derive(Default, Debug)]
pub struct Votes(pub BTreeMap<Voter, BTreeMap<ProposalIdAdapter, Vote>>);

impl From<VotingProcedures> for Votes {
    fn from(voting_procedures: VotingProcedures) -> Self {
        Self(
            voting_procedures
                .into_iter()
                .map(|(voter, votes)| {
                    (
                        voter,
                        votes
                            .into_iter()
                            .map(|(proposal, procedure)| (proposal.into(), procedure.vote))
                            .collect(),
                    )
                })
                .collect(),
        )
    }
}
