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

use std::{fmt, mem, ops::Deref};

use amaru_kernel::{
    AuxiliaryData, EraHistory, HasTransactionId, Network, NetworkId, NetworkName, ProtocolParameters, TransactionBody,
    TransactionInput, TransactionPointer, WitnessSet, cardano::value::Balance,
};
use thiserror::Error;

use crate::{
    context::ValidationContext, rules::transaction::phase_one::outputs::SupplementalDatumPolicy,
    store::GovernanceActivity,
};

pub mod certificates;
pub use certificates::InvalidCertificates;

pub mod fees;
pub use fees::InvalidFees;

pub mod inputs;
pub use inputs::InvalidInputs;

pub mod collateral;
pub use collateral::InvalidCollateral;

pub mod metadata;
pub use metadata::InvalidTransactionMetadata;

pub mod outputs;
pub use outputs::InvalidOutputs;

pub mod proposals;

pub mod vkey_witness;
pub use vkey_witness::InvalidVKeyWitness;

pub mod voting_procedures;

pub mod withdrawals;
pub use withdrawals::InvalidWithdrawals;

pub mod scripts;
pub use scripts::InvalidScripts;

pub mod native_scripts;

pub mod validity_interval;
pub use validity_interval::InvalidValidityInterval;

pub mod mint;

#[cfg(test)]
mod fixture;

#[derive(Debug, Error)]
pub enum PhaseOneError {
    #[error("invalid inputs: {0}")]
    Inputs(#[from] InvalidInputs),

    #[error("invalid outputs: {0}")]
    Outputs(#[from] InvalidOutputs),

    #[error("invalid certificates: {0}")]
    Certificates(#[from] InvalidCertificates),

    #[error("invalid fees: {0}")]
    Fees(#[from] InvalidFees),

    #[error("invalid withdrawals: {0}")]
    Withdrawals(#[from] InvalidWithdrawals),

    #[error("invalid transaction verification key witness: {0}")]
    VKeyWitness(#[from] InvalidVKeyWitness),

    #[error("invalid transaction scripts: {0}")]
    Scripts(#[from] InvalidScripts),

    #[error("invalid collateral: {0}")]
    Collateral(#[from] InvalidCollateral),

    #[error("invalid proposals: {0}")]
    Proposals(#[from] proposals::InvalidProposals),

    #[error("invalid transaction metadata: {0}")]
    Metadata(#[from] InvalidTransactionMetadata),

    #[error("invalid network ID in transaction body: expected {expected:?} provided {provided:?}")]
    InvalidNetworkID { expected: Network, provided: Network },

    #[error("transaction too large: provided {provided} bytes, maximum {maximum} bytes")]
    TooLarge { provided: u64, maximum: u64 },

    #[error("invalid transaction validity interval: {0}")]
    ValidityInterval(#[from] InvalidValidityInterval),

    #[error("value not preserved: balance = {0}")]
    ValueNotPreserved(Balance),
}

#[expect(clippy::too_many_arguments)]
pub fn execute<C>(
    context: &mut C,
    network_name: &NetworkName,
    protocol_parameters: &ProtocolParameters,
    era_history: &EraHistory,
    governance_activity: &GovernanceActivity,
    pointer: TransactionPointer,
    is_valid: bool,
    mut transaction_body: TransactionBody,
    transaction_witness_set: &WitnessSet,
    transaction_auxiliary_data: Option<&AuxiliaryData>,
    tx_size: u64,
) -> Result<Vec<TransactionInput>, PhaseOneError>
where
    C: ValidationContext + fmt::Debug,
{
    let transaction_id = transaction_body.tx_id();

    let network: Network = (*network_name).into();

    fail_on_network_mismatch(transaction_body.network_id, network)?;

    fail_on_tx_size_too_large(tx_size, protocol_parameters)?;

    validity_interval::execute(
        transaction_body.validity_interval(),
        transaction_witness_set.redeemer.is_some(),
        era_history,
        pointer.slot,
    )?;

    metadata::execute(&transaction_body, transaction_auxiliary_data, protocol_parameters.protocol_version)?;

    certificates::execute(
        context,
        network,
        protocol_parameters,
        era_history,
        governance_activity,
        pointer,
        mem::take(&mut transaction_body.certificates),
    )?;

    fees::execute(
        context,
        is_valid,
        transaction_body.fee,
        transaction_body.collateral.as_deref(),
        transaction_body.collateral_return.as_ref(),
    )?;

    inputs::execute(context, transaction_body.inputs.deref(), transaction_body.reference_inputs.as_deref())?;

    if transaction_witness_set.redeemer.is_some() {
        collateral::execute(
            context,
            transaction_body.collateral.as_deref(),
            transaction_body.collateral_return.as_ref(),
            transaction_body.total_collateral,
            transaction_body.fee,
            protocol_parameters,
        )?;
    }

    mint::execute(context, transaction_body.mint.as_ref());

    outputs::execute(
        context,
        protocol_parameters,
        network,
        mem::take(&mut transaction_body.collateral_return).map(|x| vec![x]).unwrap_or_default(),
        SupplementalDatumPolicy::Disallow,
        |_index| {
            if is_valid {
                return None;
            }

            // NOTE(1): Collateral outputs are indexed based off the number of normal outputs.
            //
            // NOTE(2): We must process collateral before processing normal outputs, or, store
            // the output length elsewhere since after having consumed the outputs, the .len()
            // will always return zero.
            let offset = transaction_body.outputs.len() as u64;
            Some(TransactionInput { transaction_id: *transaction_id.as_ref(), index: offset })
        },
    )?;

    outputs::execute(
        context,
        protocol_parameters,
        network,
        mem::take(&mut transaction_body.outputs),
        SupplementalDatumPolicy::Allow,
        |index| {
            if !is_valid {
                return None;
            }

            Some(TransactionInput { transaction_id: *transaction_id.as_ref(), index })
        },
    )?;

    withdrawals::execute(context, mem::take(&mut transaction_body.withdrawals).map(|xs| xs.to_vec()), network)?;

    proposals::execute(
        context,
        network,
        protocol_parameters,
        era_history,
        (transaction_id, pointer),
        mem::take(&mut transaction_body.proposals).map(|xs| xs.to_vec()),
    )?;

    voting_procedures::execute(context, mem::take(&mut transaction_body.votes));

    vkey_witness::execute(
        context,
        transaction_id,
        transaction_witness_set.bootstrap_witness.as_deref(),
        transaction_witness_set.vkeywitness.as_deref(),
    )?;

    scripts::execute(
        context,
        transaction_witness_set,
        transaction_body.validity_interval(),
        protocol_parameters,
        transaction_body.script_data_hash,
    )?;

    if let Some(donation) = transaction_body.donation {
        context.produce_lovelace(donation.into());
    }

    // NOTE: Value preservation
    //
    // In the case of a valid transaction, the balance must be zero.
    // However, when the transaction is invalid, this check is skipped. That is because
    // the `CollateralReturnOverflow` logic enforces value preservation for collateral return.
    if is_valid && !context.balance().is_zero() {
        return Err(PhaseOneError::ValueNotPreserved(context.balance().clone()));
    }

    // At last, consume inputs
    let consumed_inputs = if is_valid {
        transaction_body.inputs.to_vec()
    } else {
        transaction_body.collateral.map(|x| x.to_vec()).unwrap_or_default()
    };

    Ok(consumed_inputs)
}

fn fail_on_tx_size_too_large(provided: u64, protocol_parameters: &ProtocolParameters) -> Result<(), PhaseOneError> {
    let maximum = protocol_parameters.max_transaction_size;
    if provided > maximum {
        return Err(PhaseOneError::TooLarge { provided, maximum });
    }
    Ok(())
}

fn fail_on_network_mismatch(provided: Option<NetworkId>, network: Network) -> Result<(), PhaseOneError> {
    if let Some(network_id) = provided {
        let provided: Network = u8::from(network_id).into();
        if network != provided {
            return Err(PhaseOneError::InvalidNetworkID { expected: network, provided });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{
        EraHistory, ProtocolParameters, Transaction, cbor, include_json, utils::serde::FilesystemRefResolver,
    };
    use test_case::test_case;

    use super::fixture::{Expected, Fixture, Predicate};
    use crate::context::DefaultValidationContext;

    macro_rules! fixture {
        ($path:literal) => {
            include_json!(concat!("phase-one/", $path, ".json"))
        };
    }

    /// TODO: Simplify adding cases
    ///
    /// This could be created at build time via build.rs (or via a macro).
    /// The benefit to that is that:
    ///   1) This file is smaller when editing manually
    ///   2) When a new fixture is introduced, we can automatically include it
    #[test_case(fixture!("pass/simple-transfer"); "simple transfer")]
    #[test_case(fixture!("pass/with-metadata"); "with matching auxiliary data")]
    #[test_case(fixture!("fail/InvalidWitnessesUTXOW/0"); "invalid vkey signature")]
    #[test_case(fixture!("fail/MissingTxBodyMetadataHash/0"); "auxiliary data without body hash")]
    #[test_case(fixture!("fail/MissingTxMetadata/0"); "body hash without auxiliary data")]
    #[test_case(fixture!("fail/ConflictingMetadataHash/0"); "auxiliary data hash mismatch")]
    #[test_case(fixture!("fail/InputSetEmptyUTxO/0"); "empty input set")]
    #[test_case(fixture!("fail/BadInputsUTxO/0"); "unknown spent input")]
    #[test_case(fixture!("fail/BadInputsUTxO/1"); "unknown reference input")]
    #[test_case(fixture!("pass/native-script-input"); "native script-locked input")]
    #[test_case(fixture!("fail/BabbageNonDisjointRefInputs/0"); "input appears as both spent and reference")]
    #[test_case(fixture!("fail/WrongNetworkInTxBody/0"); "body network_id disagrees with fixture network")]
    #[test_case(fixture!("fail/MaxTxSizeUTxO/0"); "transaction larger than maxTransactionSize")]
    #[test_case(fixture!("fail/OutsideValidityIntervalUTxO/0"); "current slot before invalid_before")]
    #[test_case(fixture!("fail/OutsideForecast/0"); "upper validity bound past forecast horizon with redeemer")]
    #[test_case(fixture!("fail/MissingVKeyWitnessesUTXOW/0"); "vkey-locked input with empty witness set")]
    #[test_case(fixture!("fail/WrongNetworkWithdrawal/0"); "withdrawal reward account on wrong network")]
    #[test_case(fixture!("pass/withdrawal"); "withdrawal with matching credential")]
    #[test_case(fixture!("pass/validity-interval-both-bounds"); "slot strictly inside both bounds")]
    #[test_case(fixture!("pass/validity-interval-end-only"); "slot below end with no lower bound")]
    #[test_case(fixture!("fail/OutsideValidityIntervalUTxO/1"); "slot equals invalid_after exclusive upper bound")]
    #[test_case(fixture!("fail/OutsideValidityIntervalUTxO/2"); "slot equals end with no lower bound")]
    #[test_case(fixture!("pass/validity-interval-start-only"); "slot above start with no upper bound")]
    #[test_case(fixture!("pass/reference-input"); "tx with resolvable reference input")]
    #[test_case(fixture!("pass/stake-registration"); "stake credential registration cert")]
    #[test_case(fixture!("pass/mint"); "native-script mint of one asset unit")]
    fn conformance(fixture: Fixture) {
        let tx_size = fixture.transaction.len() as u64;

        let tx: Transaction = cbor::decode(&fixture.transaction).expect("decode tx");

        let resolver =
            FilesystemRefResolver::new(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/phase-one"));
        let era_history: EraHistory = fixture.era_history.resolve_into(&resolver).expect("resolve eraHistory");
        let protocol_parameters: ProtocolParameters =
            fixture.protocol_parameters.resolve(&resolver).expect("resolve protocolParameters");

        let mut ctx = DefaultValidationContext::new(fixture.initial_state.utxo);

        let result = super::execute(
            &mut ctx,
            &fixture.network,
            &protocol_parameters,
            &era_history,
            &fixture.initial_state.voting_state,
            fixture.ledger_env,
            tx.is_expected_valid,
            tx.body,
            &tx.witnesses,
            tx.auxiliary_data.as_ref(),
            tx_size,
        )
        .map_err(Predicate::from);

        match (fixture.expected, result) {
            (Expected::Pass, Ok(_)) => (),
            (Expected::Pass, Err(actual)) => panic!("expected pass, got error: {actual:?}"),
            (Expected::Fail(expected), Err(actual)) => {
                assert_eq!(actual, expected, "expected {expected:?}, got {actual:?}");
            }
            (Expected::Fail(expected), Ok(_)) => panic!("expected fail ({expected:?}), got pass"),
        }
    }
}
