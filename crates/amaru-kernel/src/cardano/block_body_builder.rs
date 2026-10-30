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

use std::collections::{BTreeMap, BTreeSet};

use amaru_minicbor_extra::{CborSizeOf, IndefiniteArray, IndefiniteMap};
use num::{BigUint, Zero};

use super::block_body_parts::{BlockBodyCommitment, BlockBodyParts, TransactionIndex, hash_body_cbor};
use crate::{
    AuxiliaryData, ExUnitPrices, ExUnits, ProtocolParameters, RationalNumber, SafeRatio, Transaction, TransactionBody,
    WitnessSet,
    cbor::WithSize,
    rational_number::{floor_to_lovelace, into_safe_ratio, safe_ratio},
    to_cbor,
    traits::HasExUnits,
};

/// A mutable block body under construction.
///
/// The builder keeps the exact serialized sizes of transaction components as they are added, so
/// it can reject a transaction before mutating the body and verify the final encoding on completion.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlockBodyBuilder {
    transaction_bodies: Vec<TransactionBody>,
    transaction_witnesses: Vec<WithSize<WitnessSet>>,
    auxiliary_data: BTreeMap<TransactionIndex, AuxiliaryData>,
    invalid_transactions: BTreeSet<TransactionIndex>,
    size: u32,
    ex_units: ExUnits,
    ref_scripts_size: u64,
}

/// Limits and fee parameters needed while selecting transactions for a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockBuilderConstraints {
    max_size: u32,
    max_ex_units: ExUnits,
    max_ref_scripts_size: u32,
    min_fee_a: u64,
    min_fee_b: u64,
    prices: ExUnitPrices,
    min_fee_ref_script_lovelace_per_byte: RationalNumber,
    max_ref_script_size_per_tx: u32,
    ref_script_cost_stride: u32,
    ref_script_cost_multiplier: RationalNumber,
}

impl BlockBuilderConstraints {
    /// Create constraints that do not impose any block-selection limit.
    ///
    /// This is only suitable while protocol parameters are unavailable; it disables the
    /// fee-based reference-script estimate until they can be supplied.
    pub fn unlimited() -> Self {
        Self {
            max_size: u32::MAX,
            max_ex_units: ExUnits::MAX,
            max_ref_scripts_size: u32::MAX,
            min_fee_a: 0,
            min_fee_b: 0,
            prices: ExUnitPrices { mem_price: RationalNumber::zero(), step_price: RationalNumber::zero() },
            min_fee_ref_script_lovelace_per_byte: RationalNumber::zero(),
            max_ref_script_size_per_tx: 0,
            ref_script_cost_stride: u32::MAX,
            ref_script_cost_multiplier: RationalNumber::one(),
        }
    }
}

impl From<&ProtocolParameters> for BlockBuilderConstraints {
    fn from(parameters: &ProtocolParameters) -> Self {
        Self {
            max_size: parameters.max_block_body_size,
            max_ex_units: parameters.max_block_ex_units,
            max_ref_scripts_size: parameters.max_ref_script_size_per_block,
            min_fee_a: parameters.min_fee_a,
            min_fee_b: parameters.min_fee_b,
            prices: parameters.prices.clone(),
            min_fee_ref_script_lovelace_per_byte: parameters.min_fee_ref_script_lovelace_per_byte,
            max_ref_script_size_per_tx: parameters.max_ref_script_size_per_tx,
            ref_script_cost_stride: parameters.ref_script_cost_stride,
            ref_script_cost_multiplier: parameters.ref_script_cost_multiplier,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, serde::Serialize, serde::Deserialize)]
pub enum BlockBodyBuilderError {
    #[error("a block cannot contain more than 65536 transactions")]
    TooManyTransactions,
    #[error("serialized block body exceeds the u32 size limit")]
    SizeOverflow,
    #[error("serialized block body size mismatch: counted {counted} bytes, encoded {encoded} bytes")]
    SizeMismatch { counted: u32, encoded: u32 },
}

impl Default for BlockBodyBuilder {
    fn default() -> Self {
        Self {
            transaction_bodies: Vec::new(),
            transaction_witnesses: Vec::new(),
            auxiliary_data: BTreeMap::new(),
            invalid_transactions: BTreeSet::new(),
            size: Self::FIXED_CBOR_OVERHEAD,
            ex_units: ExUnits { mem: 0, steps: 0 },
            ref_scripts_size: 0,
        }
    }
}

impl BlockBodyBuilder {
    const FIXED_CBOR_OVERHEAD: u32 = 8;

    pub fn size(&self) -> u32 {
        self.size
    }

    pub fn ex_units(&self) -> ExUnits {
        self.ex_units
    }

    pub fn ref_scripts_size(&self) -> u64 {
        self.ref_scripts_size
    }

    /// Build a body while deriving each transaction's reference-script bound from its fee.
    pub fn try_from_iter(
        transactions: impl IntoIterator<Item = Transaction>,
        constraints: &BlockBuilderConstraints,
    ) -> Result<Self, BlockBodyBuilderError> {
        let mut builder = Self::default();
        for transaction in transactions {
            if !builder.try_push(transaction, constraints)? {
                return Err(BlockBodyBuilderError::SizeOverflow);
            }
        }
        Ok(builder)
    }

    /// Add a transaction if all protocol block limits permit it.
    ///
    /// The builder is unchanged when this returns `Ok(false)`.
    pub fn try_push(
        &mut self,
        transaction: Transaction,
        constraints: &BlockBuilderConstraints,
    ) -> Result<bool, BlockBodyBuilderError> {
        let ref_scripts_size = ref_scripts_size_upper_bound(&transaction, constraints);
        let index = TransactionIndex::try_from(self.transaction_bodies.len())
            .map_err(|_| BlockBodyBuilderError::TooManyTransactions)?;

        let Transaction { body, witnesses, is_expected_valid, auxiliary_data } = transaction;

        // Compute sizes and check whether the transaction pushes us off limit.
        let size = u64::from(self.size)
            .checked_add(body.len())
            .and_then(|size| size.checked_add(witnesses.len() as u64))
            .and_then(|size| {
                let auxiliary_size =
                    auxiliary_data.as_ref().map(|data| index.cbor_size_of() as u64 + data.len()).unwrap_or_default();
                size.checked_add(auxiliary_size)
            })
            .and_then(|size| {
                let invalid_size = if is_expected_valid { 0 } else { index.cbor_size_of() as u64 };
                size.checked_add(invalid_size)
            })
            .ok_or(BlockBodyBuilderError::SizeOverflow)?;

        let ex_units =
            self.ex_units.checked_add(&witnesses.total_ex_units()).ok_or(BlockBodyBuilderError::SizeOverflow)?;
        let ref_scripts_size =
            self.ref_scripts_size.checked_add(ref_scripts_size).ok_or(BlockBodyBuilderError::SizeOverflow)?;

        if size > u64::from(constraints.max_size)
            || ex_units.mem > constraints.max_ex_units.mem
            || ex_units.steps > constraints.max_ex_units.steps
            || ref_scripts_size > u64::from(constraints.max_ref_scripts_size)
        {
            return Ok(false);
        }

        // Push the transaction to the block when there's space
        self.transaction_bodies.push(body);
        self.transaction_witnesses.push(witnesses);
        if let Some(auxiliary_data) = auxiliary_data {
            self.auxiliary_data.insert(index, auxiliary_data);
        }
        if !is_expected_valid {
            self.invalid_transactions.insert(index);
        }

        // Update the sizes
        self.size = size as u32;
        self.ex_units = ex_units;
        self.ref_scripts_size = ref_scripts_size;

        Ok(true)
    }

    pub fn finalize(self) -> Result<BlockBodyParts, BlockBodyBuilderError> {
        let encoded = [
            to_cbor(&IndefiniteArray(self.transaction_bodies.iter())),
            to_cbor(&IndefiniteArray(self.transaction_witnesses.iter())),
            to_cbor(&IndefiniteMap(self.auxiliary_data.iter())),
            to_cbor(&IndefiniteArray(self.invalid_transactions.iter())),
        ];

        let encoded_size = encoded.iter().try_fold(0u32, |size, part| {
            let part_size = u32::try_from(part.len()).map_err(|_| BlockBodyBuilderError::SizeOverflow)?;
            size.checked_add(part_size).ok_or(BlockBodyBuilderError::SizeOverflow)
        })?;

        if encoded_size != self.size {
            return Err(BlockBodyBuilderError::SizeMismatch { counted: self.size, encoded: encoded_size });
        }

        let commitment = BlockBodyCommitment {
            hash: hash_body_cbor([&encoded[0], &encoded[1], &encoded[2], &encoded[3]]),
            size: encoded_size,
        };

        Ok(BlockBodyParts::new(encoded, commitment))
    }
}

/// Infer a conservative reference-script size from the fee headroom.
///
/// An accepted transaction paid at least the linear transaction fee, the execution-unit fee,
/// and the tiered reference-script fee. The remaining fee therefore bounds the latter. The
/// inverse is deliberately rounded upward and capped by the per-transaction protocol limit.
///
/// The goal here is to come up with a realistic upper bound for the transaction selection for block
/// forging that is somewhat conservative but still precise enough that we don't up leaving some
/// transactions aside due reference scripts.
///
/// This inferrence happen because reference scripts are not contained in the transaction, they are
/// stored separately. So to avoid potentially many database lookup at the moment of forging to
/// resolve and measure scripts, we infer their size from the transaction fee.
fn ref_scripts_size_upper_bound(transaction: &Transaction, constraints: &BlockBuilderConstraints) -> u64 {
    let tx_size = transaction.tx_ref().len();
    let linear_fee = constraints.min_fee_a.saturating_mul(tx_size).saturating_add(constraints.min_fee_b);
    let execution_fee = plutus_execution_fee(transaction.witnesses.total_ex_units(), &constraints.prices);
    let fee_headroom = transaction.body.fee.saturating_sub(linear_fee.saturating_add(execution_fee));

    let mut lower = 0;
    let mut upper = u64::from(constraints.max_ref_script_size_per_tx) + 1;
    while lower + 1 < upper {
        let candidate = lower + (upper - lower) / 2;
        if reference_script_fee(candidate, constraints) <= fee_headroom {
            lower = candidate;
        } else {
            upper = candidate;
        }
    }
    lower
}

fn plutus_execution_fee(units: ExUnits, prices: &crate::ExUnitPrices) -> u64 {
    let mem = safe_ratio(units.mem, 1) * into_safe_ratio(&prices.mem_price);
    let steps = safe_ratio(units.steps, 1) * into_safe_ratio(&prices.step_price);
    floor_to_lovelace((mem + steps).ceil())
}

fn reference_script_fee(size: u64, constraints: &BlockBuilderConstraints) -> u64 {
    let multiplier = into_safe_ratio(&constraints.ref_script_cost_multiplier);
    assert!(constraints.ref_script_cost_stride > 0 && multiplier > SafeRatio::zero());
    let stride = BigUint::from(u64::from(constraints.ref_script_cost_stride));
    let mut rate = into_safe_ratio(&constraints.min_fee_ref_script_lovelace_per_byte);
    let mut fee = SafeRatio::zero();
    let mut remaining = BigUint::from(size);

    while remaining >= stride {
        fee += &rate * &stride;
        rate *= &multiplier;
        remaining -= &stride;
    }

    floor_to_lovelace(fee + rate * remaining)
}

#[cfg(test)]
mod tests {

    use proptest::prelude::*;

    use super::*;
    use crate::{
        EraHistory, Header, PREPROD_DEFAULT_PROTOCOL_PARAMETERS, ProtocolVersion, Redeemers, ReferenceInputs, cbor,
        utils::tests::run_strategy,
    };

    #[test]
    fn try_push_returns_false_without_mutating_when_size_limit_is_reached() {
        let mut builder = BlockBodyBuilder::default();
        let initial_size = builder.size();
        let mut constraints = BlockBuilderConstraints::unlimited();
        constraints.max_size = initial_size;
        let initial_ex_units = builder.ex_units();

        assert!(!builder.try_push(default_transaction(), &constraints).unwrap());

        assert_eq!(builder.size(), initial_size);
        assert_eq!(builder.ex_units(), initial_ex_units);
        assert_eq!(builder.finalize().unwrap().commitment().size, initial_size);
    }

    #[test]
    fn try_push_returns_false_without_mutating_when_execution_limit_is_reached() {
        let mut builder = BlockBodyBuilder::default();
        let mut constraints = BlockBuilderConstraints::unlimited();
        constraints.max_ex_units = ExUnits { mem: 0, steps: 0 };
        let initial_size = builder.size();
        let initial_ex_units = builder.ex_units();

        assert!(!builder.try_push(transaction_with_ex_units(), &constraints).unwrap());

        assert_eq!(builder.size(), initial_size);
        assert_eq!(builder.ex_units(), initial_ex_units);
        assert_eq!(builder.finalize().unwrap().commitment().size, initial_size);
    }

    #[test]
    fn try_push_tracks_auxiliary_data_and_invalid_transaction_entries() {
        let transaction = Transaction {
            auxiliary_data: Some(AuxiliaryData::default()),
            is_expected_valid: false,
            ..default_transaction()
        };
        let expected_size = BlockBodyBuilder::FIXED_CBOR_OVERHEAD
            + u32::try_from(transaction.body.len()).unwrap()
            + u32::try_from(transaction.witnesses.len()).unwrap()
            + 1
            + u32::try_from(transaction.auxiliary_data.as_ref().unwrap().len()).unwrap()
            + 1;

        let mut builder = BlockBodyBuilder::default();
        let constraints = BlockBuilderConstraints::unlimited();
        let expected_ref_scripts_size = ref_scripts_size_upper_bound(&transaction, &constraints);
        assert!(builder.try_push(transaction, &constraints).unwrap());
        assert_eq!(builder.size(), expected_size);
        assert_eq!(builder.ref_scripts_size(), expected_ref_scripts_size);

        let parts = builder.finalize().expect("tracked size matches encoded body");
        let commitment = parts.commitment();
        let mut header = run_strategy(any::<Header>());
        header.body_mut().block_body_hash = commitment.hash;
        header.body_mut().block_body_size = commitment.size;

        let block = parts
            .try_into_block(&header, &EraHistory::default())
            .expect("matching commitment")
            .decode()
            .expect("spliced block");
        assert_eq!(block.transaction_bodies.len(), 1);
        assert_eq!(block.auxiliary_data.len(), 1);
        assert!(block.invalid_transactions.contains(&0));
    }

    #[test]
    fn try_push_derives_a_reference_script_bound_from_fee_headroom() {
        let mut protocol_version = ProtocolVersion::default();
        let mut encoded_reference_input = vec![0x81, 0x82, 0x58, 0x20];
        encoded_reference_input.extend([0; 32]);
        encoded_reference_input.push(0);
        let reference_inputs: ReferenceInputs =
            cbor::from_cbor_no_leftovers_with(&encoded_reference_input, &mut protocol_version)
                .expect("valid reference input");
        let mut transaction = default_transaction();
        transaction.body.reference_inputs = Some(reference_inputs);
        transaction.body.fee = 1_000_000_000;

        let constraints = BlockBuilderConstraints::from(&*PREPROD_DEFAULT_PROTOCOL_PARAMETERS);
        let estimated = ref_scripts_size_upper_bound(&transaction, &constraints);
        assert!(estimated > 0);

        let mut builder = BlockBodyBuilder::default();
        assert!(builder.try_push(transaction, &constraints).unwrap());
        assert_eq!(builder.ref_scripts_size(), estimated);
    }

    #[test]
    fn try_push_accumulates_execution_units() {
        let transaction = transaction_with_ex_units();
        let expected = transaction.witnesses.total_ex_units();
        let mut builder = BlockBodyBuilder::default();
        let mut constraints = BlockBuilderConstraints::unlimited();
        constraints.max_ex_units = expected;

        assert!(builder.try_push(transaction, &constraints).unwrap());
        assert_eq!(builder.ex_units(), expected);
        assert_eq!(builder.ref_scripts_size(), 0);
        builder.finalize().expect("tracked size matches encoded body");
    }

    #[test]
    fn try_push_accounts_for_multi_byte_transaction_indices() {
        let transactions =
            std::iter::repeat_with(|| Transaction { is_expected_valid: false, ..default_transaction() }).take(25);

        let constraints = BlockBuilderConstraints::unlimited();
        let builder = BlockBodyBuilder::try_from_iter(transactions, &constraints).unwrap();
        let size = builder.size();
        let parts = builder.finalize().unwrap();
        assert_eq!(size, parts.commitment().size);
    }

    #[test]
    fn try_push_returns_false_when_reference_script_limit_is_reached() {
        let mut builder = BlockBodyBuilder::default();
        let initial_size = builder.size();
        let mut transaction = default_transaction();
        transaction.body.fee = 1_000_000_000;
        let mut constraints = BlockBuilderConstraints::from(&*PREPROD_DEFAULT_PROTOCOL_PARAMETERS);
        let estimated = ref_scripts_size_upper_bound(&transaction, &constraints);
        constraints.max_ref_scripts_size = u32::try_from(estimated - 1).unwrap();

        assert!(!builder.try_push(transaction, &constraints).unwrap());

        assert_eq!(builder.size(), initial_size);
        assert_eq!(builder.ref_scripts_size(), 0);
        builder.finalize().expect("unchanged builder remains valid");
    }

    #[test]
    fn finalize_rejects_an_inconsistent_tracked_size() {
        let mut builder = BlockBodyBuilder::default();
        builder.size += 1;

        assert!(matches!(builder.finalize(), Err(BlockBodyBuilderError::SizeMismatch { .. })));
    }

    #[test]
    fn finalize_commitment_matches_the_indefinite_body_encoding() {
        let parts = BlockBodyBuilder::default().finalize().unwrap();
        let commitment = parts.commitment();
        let mut header = run_strategy(any::<Header>());
        header.body_mut().block_body_hash = commitment.hash;
        header.body_mut().block_body_size = commitment.size;

        let raw = parts.try_into_block(&header, &EraHistory::default()).expect("matching commitment");
        let block = raw.decode().expect("spliced block");
        assert_eq!(block.body_hash(), commitment.hash);
        assert_eq!(block.body_len(), commitment.size as u64);
        assert_eq!(block.transaction_bodies.len(), 0);
    }

    #[test]
    fn try_into_block_rejects_a_header_with_a_different_commitment() {
        let parts = BlockBodyBuilder::default().finalize().unwrap();
        let header = run_strategy(any::<Header>());
        assert!(parts.try_into_block(&header, &EraHistory::default()).is_err());
    }

    fn default_transaction() -> Transaction {
        let body = TransactionBody::new(
            std::iter::empty::<crate::TransactionInput>(),
            std::iter::empty::<crate::MemoizedTransactionOutput>(),
            0,
        );
        let witnesses = WitnessSet::default();
        let mut protocol_version = ProtocolVersion::default();
        let witness_size = cbor::to_vec_with(&witnesses, &mut protocol_version).expect("valid witness set").len();
        Transaction {
            body,
            witnesses: WithSize::new(witnesses, witness_size),
            is_expected_valid: true,
            auxiliary_data: None,
        }
    }

    fn transaction_with_ex_units() -> Transaction {
        let mut protocol_version = ProtocolVersion::default();
        let redeemers: Redeemers =
            cbor::from_cbor_no_leftovers_with(&[0x81, 0x84, 0x00, 0x00, 0x00, 0x82, 0x07, 0x0b], &mut protocol_version)
                .expect("valid redeemer");
        let witnesses = WitnessSet { redeemer: Some(redeemers), ..WitnessSet::default() };
        let witness_size = cbor::to_vec_with(&witnesses, &mut protocol_version).expect("valid witness set").len();
        Transaction { witnesses: WithSize::new(witnesses, witness_size), ..default_transaction() }
    }
}
