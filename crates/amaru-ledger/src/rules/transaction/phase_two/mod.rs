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

use std::{collections::BTreeMap, fmt};

use amaru_kernel::{
    EraHistory, NetworkName, ProtocolParameters, TransactionBody, TransactionInput, TransactionPointer, WitnessSet,
    cbor, to_cbor, transaction_input_to_string,
};
use amaru_plutus::{
    arena_pool::ArenaPool,
    script_context::{Script, TxInfo, TxInfoTranslationError, Utxos},
    to_plutus_data::{PLUTUS_V1, PLUTUS_V2, PLUTUS_V3, PlutusDataError},
};
use amaru_uplc::{
    binder::DeBruijn,
    constant::Constant,
    data::PlutusData,
    flat::{self, FlatDecodeError},
    machine::{ExBudget, MachineInfo, PlutusVersion},
    term::Term,
};
use thiserror::Error;

use crate::context::UtxoSlice;

#[derive(Debug, Error)]
pub enum PhaseTwoError {
    #[error("missing input: {}", transaction_input_to_string(.0))]
    MissingInput(TransactionInput),
    #[error("failed to translate transaction to TxInfo: {0}")]
    TransactionTranslationError(#[from] TxInfoTranslationError),
    #[error("illegal state in ScriptContext: {0}")]
    ScriptContextStateError(#[from] PlutusDataError),
    #[error("failed to deserialize script: {0}")]
    ScriptDeserializationError(cbor::decode::Error),
    #[error("failed to flat decode script: {0}")]
    FlatDecodingError(#[from] FlatDecodeError),
    #[error("missing cost models for version: {0:?}")]
    MissingCostModel(PlutusVersion),
    #[error("script evaluation failure: {0:?}")]
    UplcMachineError(UplcMachineError),
    #[error("expected scripts to fail but didn't")]
    ValidityStateError,
}

#[derive(Debug)]
pub struct UplcMachineError {
    pub plutus_version: PlutusVersion,
    pub info: MachineInfo,
    // This should be a `MachineError`, but I'm avoiding lifetime hell for now
    pub err: String,
}

#[allow(clippy::too_many_arguments)]
pub fn execute<C>(
    context: &mut C,
    arena_pool: &ArenaPool,
    network: &NetworkName,
    protocol_parameters: &ProtocolParameters,
    era_history: &EraHistory,
    pointer: TransactionPointer,
    is_valid: bool,
    transaction_body: &TransactionBody,
    transaction_witness_set: &WitnessSet,
) -> Result<(), PhaseTwoError>
where
    C: UtxoSlice + fmt::Debug,
{
    if transaction_witness_set.redeemer.is_none() {
        return Ok(());
    }

    let mut resolved_inputs = transaction_body
        .inputs
        .into_iter()
        .map(|input| {
            Ok((input.clone(), context.lookup(input).ok_or(PhaseTwoError::MissingInput(input.clone()))?.clone()))
        })
        .collect::<Result<BTreeMap<_, _>, PhaseTwoError>>()?;

    let mut resolved_reference_inputs = transaction_body
        .reference_inputs
        .as_ref()
        .map(|reference_inputs| {
            reference_inputs
                .iter()
                .map(|input| {
                    Ok((
                        input.clone(),
                        context.lookup(input).ok_or(PhaseTwoError::MissingInput(input.clone()))?.clone(),
                    ))
                })
                .collect::<Result<BTreeMap<_, _>, PhaseTwoError>>()
        })
        .transpose()?
        .unwrap_or_default();

    resolved_inputs.append(&mut resolved_reference_inputs);
    let utxos = Utxos::from(resolved_inputs);

    let tx_info = TxInfo::new(
        transaction_body,
        transaction_witness_set,
        transaction_body.id(),
        &utxos,
        &pointer.slot,
        *network,
        era_history,
        protocol_parameters.protocol_version,
    )?;

    let scripts_to_execute = tx_info.to_script_contexts();

    let script_results = scripts_to_execute
        .into_iter()
        .map(|(script_context, script)| {
            let arena = arena_pool.acquire();
            let (args, cost_model, plutus_version) = match script {
                Script::Native(_) => {
                    unreachable!("cannot have a redeemer point to a native_script")
                }
                Script::PlutusV1(_) => (
                    script_context.to_script_args(PLUTUS_V1)?,
                    protocol_parameters
                        .cost_models
                        .plutus_v1
                        .as_deref()
                        .ok_or(PhaseTwoError::MissingCostModel(PlutusVersion::V1))?,
                    PlutusVersion::V1,
                ),
                Script::PlutusV2(_) => (
                    script_context.to_script_args(PLUTUS_V2)?,
                    protocol_parameters
                        .cost_models
                        .plutus_v2
                        .as_deref()
                        .ok_or(PhaseTwoError::MissingCostModel(PlutusVersion::V2))?,
                    amaru_uplc::machine::PlutusVersion::V2,
                ),
                Script::PlutusV3(_) => (
                    script_context.to_script_args(PLUTUS_V3)?,
                    protocol_parameters
                        .cost_models
                        .plutus_v3
                        .as_deref()
                        .ok_or(PhaseTwoError::MissingCostModel(PlutusVersion::V3))?,
                    amaru_uplc::machine::PlutusVersion::V3,
                ),
            };

            let script_bytes = script.to_bytes().map_err(PhaseTwoError::ScriptDeserializationError)?;

            let mut program = flat::decode::<DeBruijn>(
                &arena,
                &script_bytes,
                plutus_version,
                protocol_parameters.protocol_version.0 as u32,
            )
            .map_err(PhaseTwoError::from)?;

            // TODO: we should stop using Pallas' PlutusData
            // We are using Pallas' PlutusData in `amaru-plutus` and not the `PlutusData` from `uplc`
            // so we have to do this really bad conversion (uplc uses references in plutus data, so we have to make sure everything lives long enough)
            let constants = args
                .iter()
                .map(|arg| {
                    let bytes = to_cbor(&arg);
                    #[allow(clippy::expect_used)]
                    let data = PlutusData::from_cbor(&arena, &bytes).expect("unable to decode PlutusData cbor");
                    Constant::Data(data)
                })
                .collect::<Vec<_>>();
            let arguments = constants.iter().map(Term::Constant).collect::<Vec<_>>();

            for term in arguments.iter() {
                program = program.apply(&arena, term);
            }

            let budget = script_context.budget();

            let uplc_budget = ExBudget { mem: budget.mem as i64, cpu: budget.steps as i64 };

            let result = program.eval_with_params(&arena, plutus_version, cost_model, uplc_budget);

            match plutus_version {
                PlutusVersion::V1 | PlutusVersion::V2 => match result.term {
                    Ok(term) => match term {
                        Term::Error => Err(PhaseTwoError::UplcMachineError(UplcMachineError {
                            plutus_version,
                            info: result.info,
                            err: "Error term evaluated".into(),
                        })),
                        Term::Var(_)
                        | Term::Lambda { .. }
                        | Term::Apply { .. }
                        | Term::Delay(_)
                        | Term::Force(_)
                        | Term::Case { .. }
                        | Term::Constr { .. }
                        | Term::Constant(_)
                        | Term::Builtin(_) => Ok(()),
                    },
                    Err(e) => Err(PhaseTwoError::UplcMachineError(UplcMachineError {
                        plutus_version,
                        info: result.info,
                        err: e.to_string(),
                    })),
                },

                // According to [CIP-117](https://cips.cardano.org/cip/CIP-0117), Plutus V3 scripts must evaluate to a constant term of unit
                PlutusVersion::V3 => match result.term {
                    Ok(Term::Constant(Constant::Unit)) => Ok(()),
                    Ok(_) => Err(PhaseTwoError::UplcMachineError(UplcMachineError {
                        plutus_version,
                        info: result.info,
                        err: "evaluated to a non-unit term".to_string(),
                    })),
                    Err(e) => Err(PhaseTwoError::UplcMachineError(UplcMachineError {
                        plutus_version,
                        info: result.info,
                        err: e.to_string(),
                    })),
                },
            }
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|_| ());

    if is_valid {
        script_results
    } else {
        match script_results {
            Ok(_) => Err(PhaseTwoError::ValidityStateError),
            Err(_) => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::LazyLock};

    use amaru_kernel::{
        EraHistory, MemoizedTransactionOutput, NetworkName, ProtocolParameters, Transaction, TransactionPointer,
        include_cbor,
    };
    use amaru_plutus::arena_pool::ArenaPool;
    use anyhow::{Context, Result};

    use crate::context::assert::{AssertPreparationContext, AssertValidationContext};

    static ARENA_POOL: LazyLock<ArenaPool> = LazyLock::new(|| ArenaPool::new(1, 1_024_000));

    macro_rules! fixture {
        ($path:literal) => {
            include_cbor!(concat!("phase-two/mainnet/74558bb6/", $path))
        };
    }

    fn default_execute_phase_two(
        network: NetworkName,
        transaction: Transaction,
        resolved_inputs: impl Iterator<Item = MemoizedTransactionOutput>,
    ) -> Result<()> {
        let utxo = transaction
            .body
            .inputs
            .clone()
            .to_vec()
            .into_iter()
            .chain(transaction.body.reference_inputs.clone().map(|inputs| inputs.to_vec()).unwrap_or_default())
            .zip(resolved_inputs)
            .fold(BTreeMap::new(), |mut utxo, (input, output)| {
                utxo.insert(input, output);
                utxo
            });

        let mut context = AssertValidationContext::from(AssertPreparationContext { utxo });

        let protocol_parameters =
            <&ProtocolParameters>::try_from(network).map_err(anyhow::Error::msg).context("missing network defaults")?;

        super::execute(
            &mut context,
            &ARENA_POOL,
            &network,
            protocol_parameters,
            <&EraHistory>::from(network),
            default_pointer(&transaction),
            transaction.is_expected_valid,
            &transaction.body,
            &transaction.witnesses,
        )?;

        Ok(())
    }

    fn default_pointer(transaction: &Transaction) -> TransactionPointer {
        let slot = transaction
            .body
            .validity_interval_start
            .unwrap_or_default()
            .max(transaction.body.validity_interval_end.unwrap_or_default());

        TransactionPointer { slot: slot.into(), transaction_index: 0 }
    }

    #[test]
    fn golden_74558bb6() -> Result<()> {
        let resolved_inputs: [MemoizedTransactionOutput; 6] = [
            fixture!("resolved-input-0.cbor"),
            fixture!("resolved-input-1.cbor"),
            fixture!("resolved-input-2.cbor"),
            fixture!("resolved-input-3.cbor"),
            fixture!("resolved-input-4.cbor"),
            fixture!("resolved-input-5.cbor"),
        ];

        default_execute_phase_two(NetworkName::Mainnet, fixture!("tx.cbor"), resolved_inputs.into_iter())
    }
}
